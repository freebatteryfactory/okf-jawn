## Package E: Dispatch

- **Branch:** `cure/dispatch`
- **Worktree:** `D:\okf\cure\dispatch` (run every command from this directory; cargo from PowerShell only)
- **Wave:** 2, after package C is merged into `integration/foundation-cure`. Runs in parallel with D, F and G.
- **Files allowed (exact):**
  - `crates/core/src/dispatch.rs`, `crates/core/src/mutations.rs`, `crates/core/src/context.rs`, `crates/core/src/access.rs`
  - `crates/core/src/lib.rs` — module doc comments only (allowed, but this plan leaves the file untouched; see Deviations 13)
  - `crates/core/Cargo.toml` — `[dev-dependencies]` only
  - `crates/core/tests/dispatch.rs`, `crates/core/tests/authorization.rs`, `crates/core/tests/support/**`
  - `tests/support/**` (creates `tests/support/check.rs`; `tests/support/application.rs` is not changed)
- **Must not touch:** every other file. In particular `crates/core/src/{storage,jobs,credentials,sandbox,confirmations,drafts,proposals,uploads,events,conversion,search,readiness,ports}.rs` and `crates/core/src/application/mod.rs` (package F), `crates/contract/**`, `crates/server/**` (package H), `crates/mcp/**`, `xtask/**`, `Cargo.toml`, `Cargo.lock`, `clippy.toml`, `api/`, `generated/`, `crates/core/AGENTS.md`.
- **Known consequence:** this package changes the signature of `dispatch`. `crates/server` does not compile on this branch until package H. The gate is therefore scoped to `-p okf-jawn-contract -p okf-jawn-core`; never run or "fix" a workspace-wide build here.
- **SPEC / AGENTS sentences served:**
  - SPEC §8: "Every non-read mutation carries a durable MutationId under an idempotency key scoped to tenant, subject and operation. … Abandoned mutations stay until reconciled. … every store that creates a durable row … takes the MutationId and enforces it as unique, so a retry after an abandoned lease never duplicates an effect or issues a second connector secret. Typed wire errors expose AlreadyIssued, InProgress and draft/idempotency Conflict details the UI can act on."
  - SPEC §11: "Principal identity is not authorization. … effective permissions come from AccessControl as WorkspaceGrant and TenantGrant." and "idempotent retries of create_connector return AlreadyIssued without the secret."
  - SPEC §13: "Universal refusal is a product failure." and "FixtureApplication and other tests/support implementations never ship as the production application".
  - SPEC §14: "deny all/pedantic plus selected panic/unchecked-operation lints. No allow/expect/cfg_attr suppression or cap-lints workaround. … types before behavior".
  - `crates/core/AGENTS.md`: "Dispatch authorizes every `RequestScope` target, builds `OperationContext`, and runs `MutationStore::begin` before handlers."
  - Design §3: crash retry re-runs the handler under the same `MutationId`; a handler error releases the lease; the digest is independent of `preserve_order`; `Target::Authenticated`; test lints stay absolute.

### Conventions for every task in this package

- **Line numbers** are those of commit `b205c4a` (= `678f919`). Package C has since made minimum edits to keep the workspace compiling — in `crates/core/src/dispatch.rs` the 13-field macro matcher (its Task C.4) and two match arms, `Target::Authenticated => {}` and `ReplayPolicy::AlreadyIssued { .. } =>` (C.5); in `crates/core/tests/support/counting.rs` the macro matcher; in `crates/core/tests/dispatch.rs:454` one dereference — so lines are 1 to 2 higher. Locate each edit by the quoted text and item name; the line number is a hint.
- **Formatting:** never run `cargo fmt --all` without `--check` (it would rewrite other packages' files). Format only this package's files:

```powershell
rustfmt --edition 2024 crates\core\src\dispatch.rs crates\core\src\mutations.rs crates\core\src\context.rs crates\core\src\access.rs crates\core\tests\dispatch.rs crates\core\tests\authorization.rs
```

  (`rustfmt` on the two test files also formats `crates/core/tests/support/*.rs` and `tests/support/check.rs`, which they include.)
- **Commit procedure:** write the task's message text to `$env:TEMP\cure-msg.txt` (UTF-8), then

```powershell
git status --short            # must list only files from "Files allowed"
git add <the paths the task lists>
git commit -F $env:TEMP\cure-msg.txt
```

  In `Verified:` keep the command, and replace a count only with the count you observed. Never write a result you did not see.
- **Test idiom** (shared interfaces): tests return `TestResult`, use `?`, `assert!`/`assert_eq!`, `err_of`, `some`. No `unwrap`, `expect`, `expect_err`, `panic!`, `unreachable!`, `[]` indexing, `#[allow]` or `#[expect]`.
- **Module item order** (`clippy.toml`): `use`, macros, types (struct/enum/trait/alias), `const`/`static`, `impl`, `fn`, then `mod`. A `mod` declaration — including `mod check;` and `mod support;` in a test file — goes at the **end** of the file.
- **Failure classification** (AGENTS.md): a compile or lint error in this package's files is your defect — fix it in the task, without suppression. An error in another package's file is not yours: stop and report symbol, file:line and output.

### Task E.1: Test idiom and authorization tests

**Files:**
- Create: `tests/support/check.rs`
- Modify: `crates/core/tests/authorization.rs` (whole file; the violation is line 13, `.expect("tenant")`)
- Test: `tests/support/check.rs` (`mod tests`), `crates/core/tests/authorization.rs`

**Interfaces:**
- Consumes: `okf_jawn_core::access::{authorize_tenant, authorize_workspace, check_route}` (unchanged).
- Produces (exactly as in the shared interfaces):

```rust
pub type TestResult = Result<(), Box<dyn std::error::Error>>;
pub fn err_of<T: std::fmt::Debug, E>(result: Result<T, E>) -> Result<E, String>;
pub fn some<T>(value: Option<T>, what: &str) -> Result<T, String>;
```

  How a test target includes it — at the end of the test file, at the crate root, under the name `check`:

```rust
#[path = "../../../tests/support/check.rs"]
mod check;
```

  (The path is the same from `crates/<crate>/tests/<file>.rs` in every crate.) The module's own unit tests run inside every including target; that is deliberate — it keeps all three helpers "used" in every target, so an includer that never calls `some` gets no `dead_code` warning.

- [ ] **Step 1: Confirm the branch point contains package C.**

```powershell
git log -1 --oneline
Select-String -Path crates\contract\src\scope.rs -Pattern 'Authenticated', 'id_pointer'
Select-String -Path crates\contract\src\error.rs -Pattern 'NotImplemented', 'fn with_field', 'Box<ErrorDetail>'
cargo test --locked -p okf-jawn-contract -p okf-jawn-core --no-run 2>&1 | Select-Object -Last 15
```

  Expected: every pattern matches. If one does not, stop: package C is not merged. If the `--no-run` build fails **only** in `crates/core/tests/dispatch.rs` or `crates/core/tests/support/*.rs`, continue (Task E.2 replaces those files) and say so in this task's commit body. Any other compile failure: stop and report.

- [ ] **Step 2: Write the failing tests.** Create `tests/support/check.rs` containing only the `//!` header and the `#[cfg(test)] mod tests { … }` block from the listing in Step 4 (leave out `TestResult`, `err_of` and `some`). Replace `crates/core/tests/authorization.rs` with:

```rust
//! Capability and constraint are tested together, not by accepting universal refusal.

use okf_jawn_contract::access::{AccessRoute, DelegationCeiling, Permission, Principal};
use okf_jawn_contract::error::ErrorCode;
use okf_jawn_contract::identity::{IdentityError, TenantId, WorkspaceId};
use okf_jawn_core::access::{authorize_tenant, authorize_workspace, check_route};
use okf_jawn_core::context::{TenantGrant, WorkspaceGrant};
use okf_jawn_core::storage::StorageScope;

use check::{TestResult, err_of};

fn local_tenant() -> Result<TenantId, IdentityError> {
    TenantId::try_from("tenant-local".to_owned())
}

fn grant(
    workspace: WorkspaceId,
    permissions: Vec<Permission>,
) -> Result<WorkspaceGrant, IdentityError> {
    Ok(WorkspaceGrant {
        scope: StorageScope {
            tenant_id: local_tenant()?,
            workspace_id: workspace,
        },
        permissions,
    })
}

fn every_permission() -> Vec<Permission> {
    vec![
        Permission::Read,
        Permission::Propose,
        Permission::Approve,
        Permission::Review,
        Permission::Write,
        Permission::Admin,
    ]
}

#[test]
fn delegated_proposal_is_allowed_but_approval_is_not() -> TestResult {
    let workspace: WorkspaceId = serde_json::from_str("\"11111111-1111-4111-8111-111111111111\"")?;
    let principal = Principal {
        subject: "test-subject".to_owned(),
        tenant_id: local_tenant()?,
        route: AccessRoute::McpDelegation,
        client_id: Some("test-client".to_owned()),
        delegation: Some(DelegationCeiling {
            permissions: every_permission(),
            workspace_ids: Some(vec![workspace]),
        }),
    };
    let raw = grant(workspace, every_permission())?;
    for allowed in [Permission::Read, Permission::Propose] {
        let effective = authorize_workspace(&principal, raw.clone(), allowed)?;
        assert!(effective.allows(allowed));
    }
    for forbidden in [
        Permission::Approve,
        Permission::Review,
        Permission::Write,
        Permission::Admin,
    ] {
        let refused = err_of(authorize_workspace(&principal, raw.clone(), forbidden))?;
        assert_eq!(refused.code, ErrorCode::Forbidden);
    }
    let human = Principal {
        route: AccessRoute::BrowserSession,
        delegation: None,
        ..principal
    };
    check_route(&human, Permission::Approve)?;
    let approving = grant(workspace, vec![Permission::Approve])?;
    let effective = authorize_workspace(&human, approving, Permission::Approve)?;
    assert!(effective.allows(Permission::Approve));
    Ok(())
}

#[test]
fn local_owner_uses_the_same_rules_and_cannot_reach_other_workspaces() -> TestResult {
    let local: WorkspaceId = serde_json::from_str("\"11111111-1111-4111-8111-111111111111\"")?;
    let foreign: WorkspaceId = serde_json::from_str("\"33333333-3333-4333-8333-333333333333\"")?;
    let owner = Principal {
        subject: "local-installation-owner".to_owned(),
        tenant_id: local_tenant()?,
        route: AccessRoute::LocalOwner,
        client_id: None,
        delegation: None,
    };
    for permission in [
        Permission::Read,
        Permission::Write,
        Permission::Approve,
        Permission::Review,
        Permission::Admin,
    ] {
        let granted = grant(local, vec![permission])?;
        assert!(authorize_workspace(&owner, granted, permission)?.allows(permission));
        let ungranted = grant(foreign, Vec::new())?;
        let refused = err_of(authorize_workspace(&owner, ungranted, permission))?;
        assert_eq!(refused.code, ErrorCode::Forbidden);
    }
    let connector = Principal {
        subject: "local-installation-owner".to_owned(),
        tenant_id: local_tenant()?,
        route: AccessRoute::McpDelegation,
        client_id: Some("local-connector".to_owned()),
        delegation: Some(DelegationCeiling {
            permissions: vec![Permission::Read],
            workspace_ids: Some(vec![local]),
        }),
    };
    let read_and_propose = vec![Permission::Read, Permission::Propose];
    let reading = authorize_workspace(
        &connector,
        grant(local, read_and_propose.clone())?,
        Permission::Read,
    )?;
    assert_eq!(reading.permissions, vec![Permission::Read]);
    let beyond_ceiling = err_of(authorize_workspace(
        &connector,
        grant(local, read_and_propose)?,
        Permission::Propose,
    ))?;
    assert_eq!(beyond_ceiling.code, ErrorCode::Forbidden);
    let outside_workspaces = err_of(authorize_workspace(
        &connector,
        grant(foreign, vec![Permission::Read])?,
        Permission::Read,
    ))?;
    assert_eq!(outside_workspaces.code, ErrorCode::Forbidden);
    let tenant = TenantGrant {
        tenant_id: owner.tenant_id.clone(),
        permissions: vec![Permission::Admin],
    };
    assert!(authorize_tenant(&owner, tenant, Permission::Admin)?.allows(Permission::Admin));
    Ok(())
}

#[path = "../../../tests/support/check.rs"]
mod check;
```

- [ ] **Step 3: Run and watch it fail.**

```powershell
cargo test --locked -p okf-jawn-core --test authorization 2>&1 | Select-Object -Last 20
```

  Expected: compile failure with `error[E0432]` (unresolved imports) naming `super::TestResult`, `super::err_of` and `super::some` in `check.rs`, and `check::TestResult`, `check::err_of` in `authorization.rs`.

- [ ] **Step 4: Implement.** `tests/support/check.rs` in full:

```rust
//! Shared test idiom: tests return `TestResult`, use `?`, assertion macros and these helpers.
//!
//! Each test target includes this file with `#[path]` and declares it as `mod check;` at its
//! crate root, so sibling support modules can name `crate::check`. The unit tests below run in
//! every including target; they also keep each helper used, so no includer sees a dead-code
//! warning for a helper it does not call itself.

/// Result of a test that reports failure through `?` instead of panicking.
pub type TestResult = Result<(), Box<dyn std::error::Error>>;

/// The error of a result that must have failed.
///
/// # Errors
/// Returns a description of the unexpected success.
pub fn err_of<T: std::fmt::Debug, E>(result: Result<T, E>) -> Result<E, String> {
    match result {
        Ok(value) => Err(format!("expected an error, got Ok({value:?})")),
        Err(error) => Ok(error),
    }
}

/// The value of an option that must be present; `what` names it in the failure.
///
/// # Errors
/// Returns a message naming `what` when the option is empty.
pub fn some<T>(value: Option<T>, what: &str) -> Result<T, String> {
    value.ok_or_else(|| format!("expected {what} to be present"))
}

#[cfg(test)]
mod tests {
    use super::{TestResult, err_of, some};

    #[test]
    fn err_of_returns_the_error_of_a_failed_result() -> TestResult {
        let failed: Result<u8, &str> = Err("boom");
        assert_eq!(err_of(failed)?, "boom");
        Ok(())
    }

    #[test]
    fn err_of_describes_an_unexpected_success() {
        let succeeded: Result<u8, &str> = Ok(7);
        assert_eq!(
            err_of(succeeded),
            Err("expected an error, got Ok(7)".to_owned())
        );
    }

    #[test]
    fn some_returns_the_present_value() -> TestResult {
        assert_eq!(some(Some(3), "count")?, 3);
        Ok(())
    }

    #[test]
    fn some_names_the_missing_value() {
        assert_eq!(
            some(None::<u8>, "count"),
            Err("expected count to be present".to_owned())
        );
    }
}
```

- [ ] **Step 5: Run and watch it pass.**

```powershell
cargo test --locked -p okf-jawn-core --test authorization 2>&1 | Select-Object -Last 14
```

  Expected: `test result: ok. 6 passed; 0 failed` (2 authorization tests + 4 `check::tests`).

- [ ] **Step 6: Format and commit.**

```powershell
rustfmt --edition 2024 crates\core\tests\authorization.rs
git add tests/support/check.rs crates/core/tests/authorization.rs
```

```text
test(core): add the shared test idiom and convert the authorization tests.

Why: Test lints are absolute (design section 3: no unwrap, expect, panic or indexing, no suppression) and crates/core/tests/authorization.rs:13 used expect. Every later Rust test needs one blessed idiom.
What changed: tests/support/check.rs provides TestResult, err_of and some with their own unit tests. The authorization tests return TestResult and assert the refusal code instead of is_err.
Verified: cargo test --locked -p okf-jawn-core --test authorization -> 6 passed, 0 failed.
Next: Task E.2, the new dispatch surface and its fixtures.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_01LoZr4ME3crLYjcTNn5GcAC
```

### Task E.2: New dispatch surface — `Caller`, `Attempt`, a ledger without effect lookup, resumed attempts

**Files:**
- Modify: `crates/core/src/context.rs` (whole file; `OperationContext` is lines 30–43)
- Modify: `crates/core/src/mutations.rs:1-112` (module docs through the end of `trait AbandonedEffects`)
- Modify: `crates/core/src/dispatch.rs:1-143` and `:208-243` (module docs, imports, `DispatchPorts`, the macro, `prepare_mutation`, `authorize_targets`)
- Modify (replace whole file): `crates/core/tests/support/mod.rs`, `crates/core/tests/support/counting.rs`, `crates/core/tests/dispatch.rs`
- Test: `crates/core/tests/dispatch.rs`, plus the fixture self-tests inside the two support files

**Interfaces:**
- Consumes (package C): `Target::Authenticated`, `ReplayPolicy::AlreadyIssued { id_pointer }`, `ApiError { detail: Option<Box<ErrorDetail>>, .. }`, the 13-field operation tuple.
- Produces:

```rust
// crates/core/src/context.rs
pub enum Attempt { First, Resumed }                    // Debug, Clone, Copy, PartialEq, Eq
pub struct OperationContext {
    pub principal: Principal,
    pub session_id: Option<String>,
    pub operation: OperationName,
    pub tenant: Option<TenantGrant>,
    pub grants: Vec<WorkspaceGrant>,
    pub mutation: Option<MutationId>,
    pub attempt: Attempt,
}

// crates/core/src/dispatch.rs
pub struct Caller<'a> { pub principal: &'a Principal, pub session_id: Option<&'a str> }   // Debug, Clone, Copy
pub struct DispatchPorts<'a> { pub access: &'a dyn AccessControl, pub mutations: &'a dyn MutationStore }
pub async fn dispatch(
    service: &dyn Application,
    ports: &DispatchPorts<'_>,
    caller: &Caller<'_>,
    operation_id: &str,
    input: serde_json::Value,
) -> Result<serde_json::Value, ApiError>;

// crates/core/src/mutations.rs
pub trait MutationStore: Send + Sync {
    fn begin<'a>(&'a self, key: &'a MutationKey, digest: &'a Digest) -> PortFuture<'a, BeginOutcome>;
    fn complete(&self, mutation_id: MutationId, response: Value) -> PortFuture<'_, ()>;
    fn release(&self, mutation_id: MutationId) -> PortFuture<'_, ()>;
}
// deleted: MutationStore::record_effect, MutationStore::find, trait AbandonedEffects
```

  `complete` and `release` are spelled with an elided lifetime (see Deviations 1); the type is the one in the shared interfaces. Implementers must use the same spelling.

- Produces (test fixtures, reused by package H through `#[path]`):

```rust
// crates/core/tests/support/mod.rs
pub struct GrantTable { pub workspaces: BTreeMap<String, BTreeMap<WorkspaceId, Vec<Permission>>>, pub tenants: BTreeMap<String, Vec<Permission>> }
impl FixtureAccess {            // implements AccessControl
    pub fn new(table: GrantTable) -> Self;
    pub fn answer_workspaces_as(&self, scope: StorageScope) -> Result<(), ApiError>;
    pub fn answer_tenants_as(&self, tenant: TenantId) -> Result<(), ApiError>;
    pub fn lookups(&self) -> usize;
}
impl FixtureMutations {         // implements MutationStore
    pub fn new() -> Self;
    pub fn expire_leases(&self) -> Result<(), ApiError>;
    pub fn stored_body(&self, mutation_id: MutationId) -> Result<Option<Value>, ApiError>;
}
pub struct FixturePorts { pub access: Arc<FixtureAccess>, pub mutations: Arc<FixtureMutations> }
impl FixturePorts { pub fn new(table: GrantTable) -> Self; }
pub fn all_permissions() -> Vec<Permission>;
pub fn workspace(hex: &str) -> Result<WorkspaceId, serde_json::Error>;
pub fn tenant(value: &str) -> Result<TenantId, IdentityError>;
pub fn idempotency_key(hex: &str) -> Result<IdempotencyKey, serde_json::Error>;

// crates/core/tests/support/counting.rs
impl CountingApplication {      // implements Application
    pub fn new() -> Self;
    pub fn set_response(&self, operation: &str, value: Value) -> Result<(), ApiError>;
    pub fn fail_once(&self, operation: &str, error: ApiError) -> Result<(), ApiError>;
    pub fn park_next(&self, operation: &str) -> Result<(), ApiError>;
    pub async fn entered(&self);
    pub fn resume(&self);
    pub fn contexts(&self, operation: &str) -> Result<Vec<OperationContext>, ApiError>;
    pub fn call_count(&self, operation: &str) -> Result<usize, ApiError>;
}
```

  Fixture rules a later includer relies on: (1) the including target declares `mod check;` at its crate root, because the fixtures' self-tests use `crate::check`; (2) every public fixture item is exercised by a self-test in its own file, so an includer that uses only part of the fixture compiles without `dead_code`; (3) a "crash" is a dispatch future dropped while its handler is parked (`park_next` + `entered`), and lease expiry is `expire_leases` — tests never call `MutationStore::begin` themselves.

- [ ] **Step 1: Write the failing tests — fixtures.** Replace `crates/core/tests/support/mod.rs` with:

```rust
//! Fixture `AccessControl` and `MutationStore` for dispatch and binding tests; not production
//! adapters.
//!
//! The including test target declares `mod check;` (`tests/support/check.rs`) at its crate
//! root. Every public item here is exercised by the self-tests at the end of this file, so a
//! target that uses only part of the fixture still compiles without dead-code warnings.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use okf_jawn_contract::access::{Permission, Principal};
use okf_jawn_contract::error::{ApiError, ErrorCode};
use okf_jawn_contract::identity::{
    Digest, IdempotencyKey, IdentityError, MutationId, TenantId, WorkspaceId,
};
use okf_jawn_core::access::AccessControl;
use okf_jawn_core::context::{TenantGrant, WorkspaceGrant};
use okf_jawn_core::dispatch::new_mutation_id;
use okf_jawn_core::mutations::{BeginOutcome, MutationKey, MutationStore, StoredResponse};
use okf_jawn_core::ports::PortFuture;
use okf_jawn_core::storage::StorageScope;
use serde_json::Value;

/// Grant table keyed by subject.
#[derive(Debug, Default, Clone)]
pub struct GrantTable {
    /// Workspace grants: subject, then workspace, then permissions.
    pub workspaces: BTreeMap<String, BTreeMap<WorkspaceId, Vec<Permission>>>,
    /// Tenant grants: subject, then permissions.
    pub tenants: BTreeMap<String, Vec<Permission>>,
}

/// `AccessControl` built from an explicit grant table.
///
/// It answers for the scope that was asked unless a test tells it to answer for another one,
/// which is how a misbehaving adapter is simulated.
#[derive(Debug, Default)]
pub struct FixtureAccess {
    table: Mutex<GrantTable>,
    workspace_answer: Mutex<Option<StorageScope>>,
    tenant_answer: Mutex<Option<TenantId>>,
    lookups: AtomicUsize,
}

/// One ledger row: its identity, the digest it was begun with, and where it stands.
#[derive(Debug, Clone)]
struct Row {
    mutation_id: MutationId,
    digest: Digest,
    phase: Phase,
}

#[derive(Debug, Clone)]
enum Phase {
    /// A handler holds the lease until this instant.
    Leased { until: Instant },
    /// The lease was released after a handler error; the next `begin` resumes at once.
    Released,
    /// The handler finished and its response is retained.
    Completed { body: Value },
}

#[derive(Debug, Default)]
struct Ledger {
    by_key: BTreeMap<MutationKey, Row>,
    by_id: BTreeMap<MutationId, MutationKey>,
}

/// `MutationStore` over a `Mutex` map; leases use the wall clock.
#[derive(Debug)]
pub struct FixtureMutations {
    ledger: Mutex<Ledger>,
    lease: Duration,
}

/// The fixture ports one dispatch needs, shared by reference-counted handles.
pub struct FixturePorts {
    /// Grant table.
    pub access: Arc<FixtureAccess>,
    /// Mutation ledger.
    pub mutations: Arc<FixtureMutations>,
}

impl FixtureAccess {
    /// Construct from a prepared table.
    #[must_use]
    pub fn new(table: GrantTable) -> Self {
        Self {
            table: Mutex::new(table),
            ..Self::default()
        }
    }

    /// Answer every later workspace lookup with a grant for `scope`, whatever was asked.
    ///
    /// # Errors
    /// Returns when the lock is poisoned.
    pub fn answer_workspaces_as(&self, scope: StorageScope) -> Result<(), ApiError> {
        *lock(&self.workspace_answer, "workspace answer")? = Some(scope);
        Ok(())
    }

    /// Answer every later tenant lookup with a grant for `tenant`, whoever asked.
    ///
    /// # Errors
    /// Returns when the lock is poisoned.
    pub fn answer_tenants_as(&self, tenant: TenantId) -> Result<(), ApiError> {
        *lock(&self.tenant_answer, "tenant answer")? = Some(tenant);
        Ok(())
    }

    /// Number of `authorize` and `authorize_tenant` lookups made so far.
    #[must_use]
    pub fn lookups(&self) -> usize {
        self.lookups.load(Ordering::SeqCst)
    }
}

impl AccessControl for FixtureAccess {
    fn authorize<'a>(
        &'a self,
        principal: &'a Principal,
        workspace: WorkspaceId,
        _permission: Permission,
    ) -> PortFuture<'a, WorkspaceGrant> {
        Box::pin(async move {
            self.lookups.fetch_add(1, Ordering::SeqCst);
            let permissions = lock(&self.table, "grant table")?
                .workspaces
                .get(&principal.subject)
                .and_then(|granted| granted.get(&workspace))
                .cloned()
                .ok_or_else(not_granted)?;
            let scope = lock(&self.workspace_answer, "workspace answer")?
                .clone()
                .unwrap_or_else(|| StorageScope {
                    tenant_id: principal.tenant_id.clone(),
                    workspace_id: workspace,
                });
            Ok(WorkspaceGrant { scope, permissions })
        })
    }

    fn authorize_tenant<'a>(
        &'a self,
        principal: &'a Principal,
        _permission: Permission,
    ) -> PortFuture<'a, TenantGrant> {
        Box::pin(async move {
            self.lookups.fetch_add(1, Ordering::SeqCst);
            let permissions = lock(&self.table, "grant table")?
                .tenants
                .get(&principal.subject)
                .cloned()
                .ok_or_else(not_granted)?;
            let tenant_id = lock(&self.tenant_answer, "tenant answer")?
                .clone()
                .unwrap_or_else(|| principal.tenant_id.clone());
            Ok(TenantGrant {
                tenant_id,
                permissions,
            })
        })
    }

    fn grants<'a>(&'a self, principal: &'a Principal) -> PortFuture<'a, Vec<WorkspaceGrant>> {
        Box::pin(async move {
            let table = lock(&self.table, "grant table")?;
            let Some(granted) = table.workspaces.get(&principal.subject) else {
                return Ok(Vec::new());
            };
            Ok(granted
                .iter()
                .map(|(workspace_id, permissions)| WorkspaceGrant {
                    scope: StorageScope {
                        tenant_id: principal.tenant_id.clone(),
                        workspace_id: *workspace_id,
                    },
                    permissions: permissions.clone(),
                })
                .collect())
        })
    }

    fn grant_creator<'a>(
        &'a self,
        principal: &'a Principal,
        workspace: WorkspaceId,
    ) -> PortFuture<'a, WorkspaceGrant> {
        Box::pin(async move {
            lock(&self.table, "grant table")?
                .workspaces
                .entry(principal.subject.clone())
                .or_default()
                .insert(workspace, all_permissions());
            Ok(WorkspaceGrant {
                scope: StorageScope {
                    tenant_id: principal.tenant_id.clone(),
                    workspace_id: workspace,
                },
                permissions: all_permissions(),
            })
        })
    }
}

impl Ledger {
    fn row_mut(&mut self, mutation_id: MutationId) -> Result<&mut Row, ApiError> {
        let key = self.by_id.get(&mutation_id).ok_or_else(unknown_mutation)?;
        self.by_key.get_mut(key).ok_or_else(unknown_mutation)
    }
}

impl FixtureMutations {
    /// Construct an empty ledger with a 30-second lease.
    #[must_use]
    pub fn new() -> Self {
        Self {
            ledger: Mutex::new(Ledger::default()),
            lease: Duration::from_secs(30),
        }
    }

    /// Let every live lease lapse, as if its holder had crashed and the lease time had passed.
    ///
    /// # Errors
    /// Returns when the lock is poisoned.
    pub fn expire_leases(&self) -> Result<(), ApiError> {
        let now = Instant::now();
        for row in lock(&self.ledger, "mutation ledger")?.by_key.values_mut() {
            if let Phase::Leased { until } = &mut row.phase {
                *until = now;
            }
        }
        Ok(())
    }

    /// The response body retained for a completed mutation; `None` until it completes.
    ///
    /// # Errors
    /// Returns when the lock is poisoned.
    pub fn stored_body(&self, mutation_id: MutationId) -> Result<Option<Value>, ApiError> {
        let ledger = lock(&self.ledger, "mutation ledger")?;
        let Some(Phase::Completed { body }) = ledger
            .by_id
            .get(&mutation_id)
            .and_then(|key| ledger.by_key.get(key))
            .map(|row| &row.phase)
        else {
            return Ok(None);
        };
        Ok(Some(body.clone()))
    }
}

impl Default for FixtureMutations {
    fn default() -> Self {
        Self::new()
    }
}

impl MutationStore for FixtureMutations {
    fn begin<'a>(
        &'a self,
        key: &'a MutationKey,
        digest: &'a Digest,
    ) -> PortFuture<'a, BeginOutcome> {
        Box::pin(async move {
            let mut ledger = lock(&self.ledger, "mutation ledger")?;
            let now = Instant::now();
            let lease_until = now
                .checked_add(self.lease)
                .ok_or_else(|| ApiError::new(ErrorCode::Internal, "lease overflows the clock"))?;
            let Some(row) = ledger.by_key.get(key).cloned() else {
                let mutation_id = new_mutation_id();
                ledger.by_key.insert(
                    key.clone(),
                    Row {
                        mutation_id,
                        digest: digest.clone(),
                        phase: Phase::Leased { until: lease_until },
                    },
                );
                ledger.by_id.insert(mutation_id, key.clone());
                return Ok(BeginOutcome::New(mutation_id));
            };
            if row.digest != *digest {
                return Ok(BeginOutcome::Conflict {
                    operation: key.operation,
                });
            }
            let mutation_id = row.mutation_id;
            match row.phase {
                Phase::Completed { body } => {
                    Ok(BeginOutcome::Replay(StoredResponse { mutation_id, body }))
                }
                Phase::Leased { until } if now < until => Ok(BeginOutcome::InProgress {
                    mutation_id,
                    retry_after: whole_seconds(until.saturating_duration_since(now)),
                }),
                Phase::Leased { .. } | Phase::Released => {
                    ledger.by_key.insert(
                        key.clone(),
                        Row {
                            mutation_id,
                            digest: row.digest,
                            phase: Phase::Leased { until: lease_until },
                        },
                    );
                    Ok(BeginOutcome::Abandoned { mutation_id })
                }
            }
        })
    }

    fn complete(&self, mutation_id: MutationId, response: Value) -> PortFuture<'_, ()> {
        Box::pin(async move {
            let mut ledger = lock(&self.ledger, "mutation ledger")?;
            ledger.row_mut(mutation_id)?.phase = Phase::Completed { body: response };
            Ok(())
        })
    }

    fn release(&self, mutation_id: MutationId) -> PortFuture<'_, ()> {
        Box::pin(async move {
            let mut ledger = lock(&self.ledger, "mutation ledger")?;
            let row = ledger.row_mut(mutation_id)?;
            if matches!(row.phase, Phase::Leased { .. }) {
                row.phase = Phase::Released;
            }
            Ok(())
        })
    }
}

impl FixturePorts {
    /// Construct fixtures over `table` with an empty ledger.
    #[must_use]
    pub fn new(table: GrantTable) -> Self {
        Self {
            access: Arc::new(FixtureAccess::new(table)),
            mutations: Arc::new(FixtureMutations::new()),
        }
    }
}

/// Lock a fixture mutex, turning poisoning into an error instead of a panic.
fn lock<'a, T>(mutex: &'a Mutex<T>, what: &str) -> Result<MutexGuard<'a, T>, ApiError> {
    mutex
        .lock()
        .map_err(|_| ApiError::new(ErrorCode::Internal, format!("{what} lock poisoned")))
}

fn not_granted() -> ApiError {
    ApiError::new(ErrorCode::Forbidden, "Required capability is not granted")
}

fn unknown_mutation() -> ApiError {
    ApiError::new(ErrorCode::NotFound, "mutation not found")
}

/// Whole seconds a caller should wait, never less than one.
fn whole_seconds(wait: Duration) -> u32 {
    u32::try_from(wait.as_secs()).unwrap_or(u32::MAX).max(1)
}

/// Every permission, in declaration order.
#[must_use]
pub fn all_permissions() -> Vec<Permission> {
    vec![
        Permission::Read,
        Permission::Write,
        Permission::Propose,
        Permission::Approve,
        Permission::Review,
        Permission::Admin,
    ]
}

/// Parse a fixed workspace UUID used in tests.
///
/// # Errors
/// Returns when `hex` is not a UUID.
pub fn workspace(hex: &str) -> Result<WorkspaceId, serde_json::Error> {
    serde_json::from_value(Value::String(hex.to_owned()))
}

/// Parse a fixed tenant id used in tests.
///
/// # Errors
/// Returns when `value` is not a valid tenant id.
pub fn tenant(value: &str) -> Result<TenantId, IdentityError> {
    TenantId::try_from(value.to_owned())
}

/// Parse a fixed idempotency key used in tests.
///
/// # Errors
/// Returns when `hex` is not a UUID.
pub fn idempotency_key(hex: &str) -> Result<IdempotencyKey, serde_json::Error> {
    serde_json::from_value(Value::String(hex.to_owned()))
}

pub mod counting;

#[cfg(test)]
mod tests {
    use okf_jawn_contract::access::{AccessRoute, Permission, Principal};
    use okf_jawn_contract::error::ErrorCode;
    use okf_jawn_contract::identity::{Digest, IdentityError};
    use okf_jawn_contract::metadata::OperationName;
    use okf_jawn_core::access::AccessControl;
    use okf_jawn_core::mutations::{BeginOutcome, MutationKey, MutationStore};
    use okf_jawn_core::storage::StorageScope;
    use serde_json::json;

    use super::{FixturePorts, GrantTable, all_permissions, idempotency_key, tenant, workspace};
    use crate::check::{TestResult, err_of};

    const HOME: &str = "11111111-1111-4111-8111-111111111111";
    const ELSEWHERE: &str = "22222222-2222-4222-8222-222222222222";

    fn digest_of(letter: &str) -> Result<Digest, IdentityError> {
        letter.repeat(64).parse()
    }

    #[tokio::test]
    async fn fixture_ledger_leases_releases_resumes_and_replays() -> TestResult {
        let ports = FixturePorts::new(GrantTable::default());
        let ledger = ports.mutations.as_ref();
        let key = MutationKey {
            tenant_id: tenant("tenant-local")?,
            subject: "alice".to_owned(),
            operation: OperationName::CreateItem,
            key: idempotency_key("aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa")?,
        };
        let digest = digest_of("a")?;
        let BeginOutcome::New(id) = ledger.begin(&key, &digest).await? else {
            return Err("the first begin must be New".into());
        };
        assert!(matches!(
            ledger.begin(&key, &digest).await?,
            BeginOutcome::InProgress { mutation_id, retry_after }
                if mutation_id == id && retry_after >= 1
        ));
        assert!(matches!(
            ledger.begin(&key, &digest_of("b")?).await?,
            BeginOutcome::Conflict {
                operation: OperationName::CreateItem
            }
        ));

        ledger.release(id).await?;
        assert!(matches!(
            ledger.begin(&key, &digest).await?,
            BeginOutcome::Abandoned { mutation_id } if mutation_id == id
        ));
        ledger.expire_leases()?;
        assert!(matches!(
            ledger.begin(&key, &digest).await?,
            BeginOutcome::Abandoned { mutation_id } if mutation_id == id
        ));

        assert_eq!(ledger.stored_body(id)?, None);
        ledger.complete(id, json!({ "done": true })).await?;
        ledger.release(id).await?;
        assert_eq!(ledger.stored_body(id)?, Some(json!({ "done": true })));
        let BeginOutcome::Replay(stored) = ledger.begin(&key, &digest).await? else {
            return Err("a completed mutation must replay".into());
        };
        assert_eq!(stored.mutation_id, id);
        assert_eq!(stored.body, json!({ "done": true }));
        Ok(())
    }

    #[tokio::test]
    async fn fixture_access_answers_from_its_table_unless_told_otherwise() -> TestResult {
        let alice = Principal {
            subject: "alice".to_owned(),
            tenant_id: tenant("tenant-local")?,
            route: AccessRoute::LocalOwner,
            client_id: None,
            delegation: None,
        };
        let home = workspace(HOME)?;
        let elsewhere = workspace(ELSEWHERE)?;
        let mut table = GrantTable::default();
        table
            .workspaces
            .entry("alice".to_owned())
            .or_default()
            .insert(home, vec![Permission::Read]);
        table.tenants.insert("alice".to_owned(), all_permissions());
        let ports = FixturePorts::new(table);
        let access = ports.access.as_ref();

        assert_eq!(access.lookups(), 0);
        let granted = access.authorize(&alice, home, Permission::Read).await?;
        assert_eq!(granted.workspace_id(), home);
        assert_eq!(granted.permissions, vec![Permission::Read]);
        let refused = err_of(access.authorize(&alice, elsewhere, Permission::Read).await)?;
        assert_eq!(refused.code, ErrorCode::Forbidden);
        let tenant_grant = access.authorize_tenant(&alice, Permission::Admin).await?;
        assert_eq!(tenant_grant.tenant_id, alice.tenant_id);
        assert_eq!(access.lookups(), 3);

        let created = access.grant_creator(&alice, elsewhere).await?;
        assert_eq!(created.permissions, all_permissions());
        assert_eq!(access.grants(&alice).await?.len(), 2);

        access.answer_workspaces_as(StorageScope {
            tenant_id: tenant("tenant-other")?,
            workspace_id: elsewhere,
        })?;
        access.answer_tenants_as(tenant("tenant-other")?)?;
        let misrouted = access.authorize(&alice, home, Permission::Read).await?;
        assert_eq!(misrouted.workspace_id(), elsewhere);
        assert_eq!(misrouted.scope.tenant_id, tenant("tenant-other")?);
        let foreign = access.authorize_tenant(&alice, Permission::Admin).await?;
        assert_eq!(foreign.tenant_id, tenant("tenant-other")?);
        Ok(())
    }
}
```

  Replace `crates/core/tests/support/counting.rs` with:

```rust
//! Scripted `Application` for dispatch and binding tests.
//!
//! It records every handler call with the context dispatch built, and answers from a configured
//! response, a one-shot error, or a handler parked until the test resumes it.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Mutex;

use okf_jawn_contract::error::{ApiError, ErrorCode};
use okf_jawn_core::context::OperationContext;
use okf_jawn_core::ports::{Application, PortFuture};
use serde_json::Value;
use tokio::sync::Notify;

use super::lock;

macro_rules! counting_operations {
    ($(($id:ident, $request:ty, $response:ty, $path:literal, $label:literal, $alias:literal,
        $operator:literal, $visibility:literal, $permission:ident, $ui:literal, $status:literal,
        $destructive:literal, $description:literal)),* $(,)?) => {
        impl Application for CountingApplication {
            $(fn $id<'a>(&'a self, context: &'a OperationContext, _request: $request) -> PortFuture<'a, $response> {
                Box::pin(async move {
                    let value = self.run(stringify!($id), context).await?;
                    serde_json::from_value(value)
                        .map_err(|error| ApiError::new(ErrorCode::Internal, error.to_string()))
                })
            })*
        }
    };
}

/// `Application` that records handler calls and plays a per-operation script.
#[derive(Default)]
pub struct CountingApplication {
    responses: Mutex<BTreeMap<String, Value>>,
    failures: Mutex<BTreeMap<String, ApiError>>,
    parked: Mutex<BTreeSet<String>>,
    calls: Mutex<Vec<(String, OperationContext)>>,
    entered: Notify,
    resumed: Notify,
}

impl CountingApplication {
    /// Construct with no configured responses.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Configure the successful JSON response of one operation.
    ///
    /// # Errors
    /// Returns when the lock is poisoned.
    pub fn set_response(&self, operation: &str, value: Value) -> Result<(), ApiError> {
        lock(&self.responses, "responses")?.insert(operation.to_owned(), value);
        Ok(())
    }

    /// Make the next call of `operation` fail with `error`; later calls answer normally.
    ///
    /// # Errors
    /// Returns when the lock is poisoned.
    pub fn fail_once(&self, operation: &str, error: ApiError) -> Result<(), ApiError> {
        lock(&self.failures, "failures")?.insert(operation.to_owned(), error);
        Ok(())
    }

    /// Make the next call of `operation` wait inside its handler until [`Self::resume`].
    ///
    /// # Errors
    /// Returns when the lock is poisoned.
    pub fn park_next(&self, operation: &str) -> Result<(), ApiError> {
        lock(&self.parked, "parked operations")?.insert(operation.to_owned());
        Ok(())
    }

    /// Resolve once a parked handler has been entered.
    pub async fn entered(&self) {
        self.entered.notified().await;
    }

    /// Let the parked handler continue.
    pub fn resume(&self) {
        self.resumed.notify_one();
    }

    /// The context of every call of `operation`, oldest first.
    ///
    /// # Errors
    /// Returns when the lock is poisoned.
    pub fn contexts(&self, operation: &str) -> Result<Vec<OperationContext>, ApiError> {
        Ok(lock(&self.calls, "calls")?
            .iter()
            .filter(|(name, _)| name == operation)
            .map(|(_, context)| context.clone())
            .collect())
    }

    /// Number of times the handler of `operation` ran.
    ///
    /// # Errors
    /// Returns when the lock is poisoned.
    pub fn call_count(&self, operation: &str) -> Result<usize, ApiError> {
        Ok(self.contexts(operation)?.len())
    }

    async fn run(&self, operation: &str, context: &OperationContext) -> Result<Value, ApiError> {
        lock(&self.calls, "calls")?.push((operation.to_owned(), context.clone()));
        let parked = lock(&self.parked, "parked operations")?.remove(operation);
        if parked {
            self.entered.notify_one();
            self.resumed.notified().await;
        }
        let failure = lock(&self.failures, "failures")?.remove(operation);
        if let Some(error) = failure {
            return Err(error);
        }
        lock(&self.responses, "responses")?
            .get(operation)
            .cloned()
            .ok_or_else(|| ApiError::new(ErrorCode::Unavailable, "No test response configured"))
    }
}

okf_jawn_contract::for_each_operation!(counting_operations);

#[cfg(test)]
mod tests {
    use okf_jawn_contract::access::{AccessRoute, Principal};
    use okf_jawn_contract::error::{ApiError, ErrorCode};
    use okf_jawn_contract::health::HealthRequest;
    use okf_jawn_contract::identity::{IdentityError, TenantId};
    use okf_jawn_contract::metadata::OperationName;
    use okf_jawn_core::context::{Attempt, OperationContext};
    use okf_jawn_core::ports::Application;
    use serde_json::json;

    use super::CountingApplication;
    use crate::check::{TestResult, err_of};

    fn context() -> Result<OperationContext, IdentityError> {
        Ok(OperationContext {
            principal: Principal {
                subject: "alice".to_owned(),
                tenant_id: TenantId::try_from("tenant-local".to_owned())?,
                route: AccessRoute::LocalOwner,
                client_id: None,
                delegation: None,
            },
            session_id: None,
            operation: OperationName::GetHealth,
            tenant: None,
            grants: Vec::new(),
            mutation: None,
            attempt: Attempt::First,
        })
    }

    #[tokio::test]
    async fn scripted_handler_records_calls_fails_once_then_answers() -> TestResult {
        let app = CountingApplication::new();
        let context = context()?;
        let unconfigured = err_of(app.get_health(&context, HealthRequest {}).await)?;
        assert_eq!(unconfigured.code, ErrorCode::Unavailable);

        app.set_response(
            "get_health",
            json!({ "status": "alive", "version": "test" }),
        )?;
        app.fail_once("get_health", ApiError::new(ErrorCode::Conflict, "scripted"))?;
        let scripted = err_of(app.get_health(&context, HealthRequest {}).await)?;
        assert_eq!(scripted.code, ErrorCode::Conflict);
        let answered = app.get_health(&context, HealthRequest {}).await?;
        assert_eq!(answered.status, "alive");

        assert_eq!(app.call_count("get_health")?, 3);
        assert_eq!(app.call_count("list_items")?, 0);
        let recorded = app.contexts("get_health")?;
        assert_eq!(
            recorded.first().map(|seen| seen.attempt),
            Some(Attempt::First)
        );
        Ok(())
    }

    #[tokio::test]
    async fn parked_handler_waits_until_it_is_resumed() -> TestResult {
        let app = CountingApplication::new();
        let context = context()?;
        app.set_response(
            "get_health",
            json!({ "status": "alive", "version": "test" }),
        )?;
        app.park_next("get_health")?;
        let mut call = app.get_health(&context, HealthRequest {});
        tokio::select! {
            biased;
            outcome = &mut call => {
                return Err(format!("the handler finished while parked: {outcome:?}").into());
            }
            () = app.entered() => {}
        }
        app.resume();
        assert_eq!(call.await?.status, "alive");
        Ok(())
    }
}
```

- [ ] **Step 2: Write the failing tests — dispatch.** Replace `crates/core/tests/dispatch.rs` with (the `mod` lines are last on purpose):

```rust
//! Dispatch tests: every case goes through `dispatch` with the fixture `AccessControl` and the
//! fixture `MutationStore`, and fails when the rule it names is removed.

use okf_jawn_contract::access::{AccessRoute, DelegationCeiling, Permission, Principal};
use okf_jawn_contract::error::{ApiError, ErrorCode, ErrorDetail};
use okf_jawn_contract::identity::IdentityError;
use okf_jawn_core::context::Attempt;
use okf_jawn_core::dispatch::{Caller, DispatchPorts, dispatch};
use serde_json::{Value, json};

use check::{TestResult, err_of, some};
use support::counting::CountingApplication;
use support::{FixturePorts, GrantTable, all_permissions, tenant, workspace};

const WORKSPACE_A: &str = "11111111-1111-4111-8111-111111111111";
const WORKSPACE_B: &str = "22222222-2222-4222-8222-222222222222";
const WORKSPACE_C: &str = "33333333-3333-4333-8333-333333333333";
const KEY_ONE: &str = "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa";
const KEY_TWO: &str = "bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb";
const REVISION: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const CONNECTOR: &str = "cccccccc-cccc-4ccc-8ccc-cccccccccccc";

fn principal(subject: &str, route: AccessRoute) -> Result<Principal, IdentityError> {
    Ok(Principal {
        subject: subject.to_owned(),
        tenant_id: tenant("tenant-local")?,
        route,
        client_id: None,
        delegation: None,
    })
}

/// A connector acting for `subject` under a delegation ceiling of `permissions`.
fn connector(subject: &str, permissions: Vec<Permission>) -> Result<Principal, IdentityError> {
    let mut delegated = principal(subject, AccessRoute::McpDelegation)?;
    delegated.client_id = Some("connector".to_owned());
    delegated.delegation = Some(DelegationCeiling {
        permissions,
        workspace_ids: None,
    });
    Ok(delegated)
}

/// Alice holds every permission on A and on the tenant, and only Read on B.
fn ports_admin_a_read_b() -> Result<FixturePorts, serde_json::Error> {
    let mut table = GrantTable::default();
    let alice = table.workspaces.entry("alice".to_owned()).or_default();
    alice.insert(workspace(WORKSPACE_A)?, all_permissions());
    alice.insert(workspace(WORKSPACE_B)?, vec![Permission::Read]);
    table.tenants.insert("alice".to_owned(), all_permissions());
    Ok(FixturePorts::new(table))
}

fn dispatch_ports(ports: &FixturePorts) -> DispatchPorts<'_> {
    DispatchPorts {
        access: ports.access.as_ref(),
        mutations: ports.mutations.as_ref(),
    }
}

/// Dispatch as a caller without a browser session.
async fn call(
    app: &CountingApplication,
    ports: &FixturePorts,
    principal: &Principal,
    operation: &str,
    input: Value,
) -> Result<Value, ApiError> {
    let caller = Caller {
        principal,
        session_id: None,
    };
    dispatch(app, &dispatch_ports(ports), &caller, operation, input).await
}

/// Start `operation`, wait until its handler is running, then drop the attempt: a crash that
/// leaves the lease behind.
async fn crash_in_handler(
    app: &CountingApplication,
    ports: &FixturePorts,
    principal: &Principal,
    operation: &str,
    input: Value,
) -> TestResult {
    app.park_next(operation)?;
    let attempt = call(app, ports, principal, operation, input);
    tokio::pin!(attempt);
    tokio::select! {
        biased;
        outcome = &mut attempt => Err(Box::<dyn std::error::Error>::from(format!(
            "the attempt finished while its handler was parked: {outcome:?}"
        ))),
        () = app.entered() => Ok(()),
    }
}

fn create_item_body(workspace_id: &str, key: &str, text: &str) -> Value {
    json!({
        "workspace_id": workspace_id,
        "base_revision": REVISION,
        "path": "notes/a.md",
        "title": "a",
        "type_name": "note",
        "kind": "note",
        "body": text,
        "properties": {},
        "idempotency_key": key
    })
}

fn item_document(text: &str) -> Value {
    json!({
        "summary": {
            "id": "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa",
            "path": "notes/a.md",
            "title": "a",
            "description": "",
            "type_name": "note",
            "kind": "note",
            "revision": REVISION,
            "lifecycle": "active"
        },
        "body": text,
        "properties": {}
    })
}

fn list_items_body(workspace_id: &str) -> Value {
    json!({
        "workspace_id": workspace_id,
        "at": { "kind": "latest" },
        "folder": "",
        "page": { "limit": 10 }
    })
}

fn listing() -> Value {
    json!({ "revision": REVISION, "items": [], "folders": [] })
}

fn create_workspace_body(key: &str) -> Value {
    json!({ "name": "n", "description": "d", "idempotency_key": key })
}

fn created_workspace() -> Value {
    json!({
        "id": WORKSPACE_A,
        "name": "n",
        "description": "d",
        "head": REVISION,
        "created_at": "2026-01-01T00:00:00Z",
        "permissions": ["admin"]
    })
}

fn open_proposal_body(key: &str) -> Value {
    json!({
        "workspace_id": WORKSPACE_A,
        "base_revision": REVISION,
        "title": "t",
        "description": "d",
        "changes": [],
        "idempotency_key": key
    })
}

fn proposal() -> Value {
    json!({
        "id": "dddddddd-dddd-4ddd-8ddd-dddddddddddd",
        "workspace_id": WORKSPACE_A,
        "base_revision": REVISION,
        "proposal_revision": REVISION,
        "content_digest": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        "title": "t",
        "description": "d",
        "changes": [],
        "status": "open",
        "created_by": "alice",
        "created_at": "2026-01-01T00:00:00Z"
    })
}

fn create_connector_body(key: &str) -> Value {
    json!({
        "label": "agent",
        "workspace_ids": [WORKSPACE_A],
        "allow_propose": false,
        "idempotency_key": key
    })
}

fn issued_connector() -> Value {
    json!({
        "connector": {
            "connector_id": CONNECTOR,
            "label": "agent",
            "workspace_ids": [WORKSPACE_A],
            "permissions": ["read"],
            "created_at": "2026-01-01T00:00:00Z"
        },
        "secret": "super-secret-value"
    })
}

#[tokio::test]
async fn write_needs_a_write_grant_and_read_needs_only_read() -> TestResult {
    let ports = ports_admin_a_read_b()?;
    let app = CountingApplication::new();
    app.set_response("create_item", item_document("hello"))?;
    app.set_response("list_items", listing())?;
    let alice = principal("alice", AccessRoute::LocalOwner)?;

    let body = create_item_body(WORKSPACE_A, KEY_ONE, "hello");
    let created = call(&app, &ports, &alice, "create_item", body).await?;
    assert_eq!(created.get("body"), Some(&json!("hello")));
    assert_eq!(app.call_count("create_item")?, 1);

    let body = create_item_body(WORKSPACE_B, KEY_TWO, "hello");
    let refused = err_of(call(&app, &ports, &alice, "create_item", body).await)?;
    assert_eq!(refused.code, ErrorCode::Forbidden);
    assert_eq!(app.call_count("create_item")?, 1);

    let listed = call(
        &app,
        &ports,
        &alice,
        "list_items",
        list_items_body(WORKSPACE_B),
    )
    .await?;
    assert_eq!(listed.get("items"), Some(&json!([])));
    assert_eq!(app.call_count("list_items")?, 1);
    Ok(())
}

#[tokio::test]
async fn present_with_a_foreign_binding_is_forbidden_before_the_handler() -> TestResult {
    let ports = ports_admin_a_read_b()?;
    let app = CountingApplication::new();
    let alice = principal("alice", AccessRoute::LocalOwner)?;
    let input = json!({
        "workspace_id": WORKSPACE_A,
        "view": {
            "schema_version": 1,
            "title": "t",
            "description": "d",
            "mode": "pinned",
            "grammar": "json_render",
            "bindings": [{
                "name": "src",
                "source": {
                    "workspace_id": WORKSPACE_C,
                    "item_id": "cccccccc-cccc-4ccc-8ccc-cccccccccccc",
                    "path": "notes/a.md",
                    "revision": REVISION,
                    "selection": { "kind": "all" }
                },
                "units": {},
                "transforms": []
            }],
            "spec": {},
            "charts": {}
        }
    });
    let refused = err_of(call(&app, &ports, &alice, "present_view", input).await)?;
    assert_eq!(refused.code, ErrorCode::Forbidden);
    assert_eq!(app.call_count("present_view")?, 0);
    Ok(())
}

#[tokio::test]
async fn create_workspace_without_a_tenant_grant_is_forbidden() -> TestResult {
    let mut table = GrantTable::default();
    table
        .workspaces
        .entry("alice".to_owned())
        .or_default()
        .insert(workspace(WORKSPACE_A)?, all_permissions());
    let ports = FixturePorts::new(table);
    let app = CountingApplication::new();
    app.set_response("create_workspace", created_workspace())?;
    let alice = principal("alice", AccessRoute::LocalOwner)?;
    let input = create_workspace_body(KEY_ONE);
    let refused = err_of(call(&app, &ports, &alice, "create_workspace", input).await)?;
    assert_eq!(refused.code, ErrorCode::Forbidden);
    assert_eq!(app.call_count("create_workspace")?, 0);
    Ok(())
}

#[tokio::test]
async fn delegation_ceiling_decides_whether_a_connector_may_propose() -> TestResult {
    let ports = ports_admin_a_read_b()?;
    let app = CountingApplication::new();
    app.set_response("open_proposal", proposal())?;

    // Proposing passes the route check for a connector, and Alice's raw grant on A includes
    // Propose, so the ceiling is the only rule left to refuse a read-only connector.
    let reader = connector("alice", vec![Permission::Read])?;
    let input = open_proposal_body(KEY_ONE);
    let refused = err_of(call(&app, &ports, &reader, "open_proposal", input).await)?;
    assert_eq!(refused.code, ErrorCode::Forbidden);
    assert_eq!(app.call_count("open_proposal")?, 0);

    let drafter = connector("alice", vec![Permission::Read, Permission::Propose])?;
    let input = open_proposal_body(KEY_TWO);
    let opened = call(&app, &ports, &drafter, "open_proposal", input).await?;
    assert_eq!(opened.get("status"), Some(&json!("open")));
    assert_eq!(app.call_count("open_proposal")?, 1);

    // A propose-capable ceiling still never reaches a write.
    app.set_response("create_item", item_document("hello"))?;
    let body = create_item_body(WORKSPACE_A, KEY_ONE, "hello");
    let refused = err_of(call(&app, &ports, &drafter, "create_item", body).await)?;
    assert_eq!(refused.code, ErrorCode::Forbidden);
    assert_eq!(app.call_count("create_item")?, 0);
    Ok(())
}

#[tokio::test]
async fn replay_returns_the_stored_response_and_the_handler_runs_once() -> TestResult {
    let ports = ports_admin_a_read_b()?;
    let app = CountingApplication::new();
    app.set_response("create_item", item_document("hello"))?;
    let alice = principal("alice", AccessRoute::LocalOwner)?;
    let body = create_item_body(WORKSPACE_A, KEY_ONE, "hello");
    let created = call(&app, &ports, &alice, "create_item", body.clone()).await?;
    let replayed = call(&app, &ports, &alice, "create_item", body).await?;
    assert_eq!(replayed, created);
    assert_eq!(app.call_count("create_item")?, 1);
    Ok(())
}

#[tokio::test]
async fn the_same_key_with_a_different_body_conflicts() -> TestResult {
    let ports = ports_admin_a_read_b()?;
    let app = CountingApplication::new();
    app.set_response("create_item", item_document("hello"))?;
    let alice = principal("alice", AccessRoute::LocalOwner)?;
    let body = create_item_body(WORKSPACE_A, KEY_ONE, "hello");
    call(&app, &ports, &alice, "create_item", body).await?;
    let changed = create_item_body(WORKSPACE_A, KEY_ONE, "different");
    let refused = err_of(call(&app, &ports, &alice, "create_item", changed).await)?;
    assert_eq!(refused.code, ErrorCode::Conflict);
    assert_eq!(app.call_count("create_item")?, 1);
    Ok(())
}

#[tokio::test]
async fn a_second_attempt_during_a_live_lease_is_in_progress() -> TestResult {
    let ports = ports_admin_a_read_b()?;
    let app = CountingApplication::new();
    app.set_response("create_item", item_document("hello"))?;
    app.park_next("create_item")?;
    let alice = principal("alice", AccessRoute::LocalOwner)?;
    let body = create_item_body(WORKSPACE_A, KEY_ONE, "hello");
    let running = call(&app, &ports, &alice, "create_item", body.clone());
    tokio::pin!(running);
    tokio::select! {
        biased;
        outcome = &mut running => {
            return Err(format!("the first attempt finished while parked: {outcome:?}").into());
        }
        () = app.entered() => {}
    }

    let refused = err_of(call(&app, &ports, &alice, "create_item", body).await)?;
    assert_eq!(refused.code, ErrorCode::InProgress);
    let seen = app.contexts("create_item")?;
    let holder = some(
        seen.first().and_then(|context| context.mutation),
        "the mutation id of the running attempt",
    )?;
    let detail = some(refused.detail, "the in-progress detail")?;
    assert!(matches!(
        *detail,
        ErrorDetail::InProgress { mutation_id, retry_after }
            if mutation_id == holder && retry_after >= 1
    ));
    assert_eq!(app.call_count("create_item")?, 1);

    app.resume();
    let finished = running.await?;
    assert_eq!(finished.get("body"), Some(&json!("hello")));
    Ok(())
}

#[tokio::test]
async fn an_abandoned_attempt_reruns_the_handler_once_as_resumed() -> TestResult {
    let ports = ports_admin_a_read_b()?;
    let app = CountingApplication::new();
    app.set_response("create_item", item_document("hello"))?;
    let alice = principal("alice", AccessRoute::LocalOwner)?;
    let body = create_item_body(WORKSPACE_A, KEY_ONE, "hello");
    crash_in_handler(&app, &ports, &alice, "create_item", body.clone()).await?;
    ports.mutations.expire_leases()?;

    let response = call(&app, &ports, &alice, "create_item", body.clone()).await?;
    assert_eq!(response.get("body"), Some(&json!("hello")));
    let seen = app.contexts("create_item")?;
    assert_eq!(seen.len(), 2);
    let crashed = some(seen.first(), "the crashed attempt")?;
    let resumed = some(seen.get(1), "the resumed attempt")?;
    assert_eq!(crashed.attempt, Attempt::First);
    assert_eq!(resumed.attempt, Attempt::Resumed);
    assert!(crashed.mutation.is_some());
    assert_eq!(resumed.mutation, crashed.mutation);

    // The resumed attempt completed the mutation, so one more retry replays without a handler.
    let replayed = call(&app, &ports, &alice, "create_item", body).await?;
    assert_eq!(replayed, response);
    assert_eq!(app.call_count("create_item")?, 2);
    Ok(())
}

#[tokio::test]
async fn a_resumed_create_connector_stores_only_the_connector_id() -> TestResult {
    let ports = ports_admin_a_read_b()?;
    let app = CountingApplication::new();
    app.set_response("create_connector", issued_connector())?;
    let alice = principal("alice", AccessRoute::LocalOwner)?;
    let input = create_connector_body(KEY_ONE);
    crash_in_handler(&app, &ports, &alice, "create_connector", input.clone()).await?;
    ports.mutations.expire_leases()?;

    let issued = call(&app, &ports, &alice, "create_connector", input).await?;
    assert_eq!(issued.get("secret"), Some(&json!("super-secret-value")));
    let seen = app.contexts("create_connector")?;
    let resumed = some(seen.get(1), "the resumed attempt")?;
    assert_eq!(resumed.attempt, Attempt::Resumed);
    let mutation_id = some(resumed.mutation, "the mutation id")?;
    assert_eq!(
        ports.mutations.stored_body(mutation_id)?,
        Some(json!({ "connector_id": CONNECTOR }))
    );
    Ok(())
}

#[tokio::test]
async fn create_connector_replay_is_already_issued_without_the_secret() -> TestResult {
    let ports = ports_admin_a_read_b()?;
    let app = CountingApplication::new();
    app.set_response("create_connector", issued_connector())?;
    let alice = principal("alice", AccessRoute::LocalOwner)?;
    let input = create_connector_body(KEY_ONE);
    let issued = call(&app, &ports, &alice, "create_connector", input.clone()).await?;
    assert_eq!(issued.get("secret"), Some(&json!("super-secret-value")));

    let refused = err_of(call(&app, &ports, &alice, "create_connector", input).await)?;
    assert_eq!(refused.code, ErrorCode::AlreadyIssued);
    assert_eq!(app.call_count("create_connector")?, 1);
    let detail = some(refused.detail, "the already-issued detail")?;
    assert!(matches!(
        *detail,
        ErrorDetail::AlreadyIssued { connector_id } if connector_id.0.to_string() == CONNECTOR
    ));

    let seen = app.contexts("create_connector")?;
    let mutation_id = some(
        seen.first().and_then(|context| context.mutation),
        "the mutation id",
    )?;
    assert_eq!(
        ports.mutations.stored_body(mutation_id)?,
        Some(json!({ "connector_id": CONNECTOR }))
    );
    Ok(())
}

#[tokio::test]
async fn the_same_key_from_two_subjects_is_two_mutations() -> TestResult {
    let mut table = GrantTable::default();
    for subject in ["xavier", "yolanda"] {
        table
            .workspaces
            .entry(subject.to_owned())
            .or_default()
            .insert(workspace(WORKSPACE_A)?, all_permissions());
    }
    let ports = FixturePorts::new(table);
    let app = CountingApplication::new();
    let body = create_item_body(WORKSPACE_A, KEY_ONE, "hello");

    app.set_response("create_item", item_document("for xavier"))?;
    let xavier = principal("xavier", AccessRoute::LocalOwner)?;
    let xavier_item = call(&app, &ports, &xavier, "create_item", body.clone()).await?;
    app.set_response("create_item", item_document("for yolanda"))?;
    let yolanda = principal("yolanda", AccessRoute::LocalOwner)?;
    let yolanda_item = call(&app, &ports, &yolanda, "create_item", body).await?;

    assert_eq!(xavier_item.get("body"), Some(&json!("for xavier")));
    assert_eq!(yolanda_item.get("body"), Some(&json!("for yolanda")));
    assert_eq!(app.call_count("create_item")?, 2);
    Ok(())
}

#[tokio::test]
async fn a_write_without_an_idempotency_key_is_rejected_by_the_schema() -> TestResult {
    let ports = ports_admin_a_read_b()?;
    let app = CountingApplication::new();
    let alice = principal("alice", AccessRoute::LocalOwner)?;
    let input = json!({
        "workspace_id": WORKSPACE_A,
        "base_revision": REVISION,
        "path": "notes/a.md",
        "title": "a",
        "type_name": "note",
        "kind": "note",
        "body": "hello",
        "properties": {}
    });
    let refused = err_of(call(&app, &ports, &alice, "create_item", input).await)?;
    assert_eq!(refused.code, ErrorCode::InvalidInput);
    assert_eq!(app.call_count("create_item")?, 0);
    Ok(())
}

#[tokio::test]
async fn an_authenticated_target_needs_no_grant_and_no_adapter_lookup() -> TestResult {
    let ports = FixturePorts::new(GrantTable::default());
    let app = CountingApplication::new();
    app.set_response("list_workspaces", json!({ "items": [] }))?;
    let stranger = principal("stranger", AccessRoute::BrowserSession)?;
    let input = json!({ "page": { "limit": 10 } });
    let listed = call(&app, &ports, &stranger, "list_workspaces", input).await?;
    assert_eq!(listed.get("items"), Some(&json!([])));
    assert_eq!(ports.access.lookups(), 0);
    let seen = app.contexts("list_workspaces")?;
    let context = some(seen.first(), "the handler context")?;
    assert!(context.tenant.is_none());
    assert!(context.grants.is_empty());
    Ok(())
}

#[tokio::test]
async fn the_session_id_of_the_caller_reaches_the_handler() -> TestResult {
    let ports = ports_admin_a_read_b()?;
    let app = CountingApplication::new();
    app.set_response("list_items", listing())?;
    let alice = principal("alice", AccessRoute::BrowserSession)?;
    let in_session = Caller {
        principal: &alice,
        session_id: Some("session-1"),
    };
    dispatch(
        &app,
        &dispatch_ports(&ports),
        &in_session,
        "list_items",
        list_items_body(WORKSPACE_A),
    )
    .await?;
    call(
        &app,
        &ports,
        &alice,
        "list_items",
        list_items_body(WORKSPACE_A),
    )
    .await?;

    let seen = app.contexts("list_items")?;
    let with_session = some(seen.first(), "the call made inside a session")?;
    let without_session = some(seen.get(1), "the call made without a session")?;
    assert_eq!(with_session.session_id.as_deref(), Some("session-1"));
    assert_eq!(with_session.attempt, Attempt::First);
    assert_eq!(without_session.session_id, None);
    Ok(())
}

#[path = "../../../tests/support/check.rs"]
mod check;
mod support;
```

- [ ] **Step 3: Run and watch it fail.**

```powershell
cargo test --locked -p okf-jawn-core --test dispatch 2>&1 | Select-String -Pattern '^error' | Select-Object -First 12
```

  Expected: compile failure. The errors include `E0432` (unresolved import) for `okf_jawn_core::dispatch::Caller` and `okf_jawn_core::context::Attempt`, and `E0407` (method is not a member of trait) for `release` on `MutationStore`.

- [ ] **Step 4: Implement `context.rs`.** Replace `crates/core/src/context.rs` with:

```rust
//! Per-request authorization and mutation identity assembled by dispatch.
//!
//! Identity (`Principal`) is not authorization. Effective permissions arrive as grants;
//! every mutation that runs carries a durable `MutationId`, and a handler is told whether an
//! earlier attempt under that id may already have written rows.

use okf_jawn_contract::access::{Permission, Principal};
use okf_jawn_contract::identity::{MutationId, TenantId, WorkspaceId};
use okf_jawn_contract::metadata::OperationName;

use crate::storage::StorageScope;

/// Effective permissions on one workspace after route and delegation rules.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceGrant {
    /// Storage scope the grant authorizes.
    pub scope: StorageScope,
    /// Capabilities the caller may exercise in this workspace.
    pub permissions: Vec<Permission>,
}

/// Effective permissions on the caller's tenant (deployment-scoped operations).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TenantGrant {
    /// Tenant boundary this grant covers.
    pub tenant_id: TenantId,
    /// Capabilities the caller may exercise at tenant scope.
    pub permissions: Vec<Permission>,
}

/// Whether a handler is the first to run under its `MutationId`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Attempt {
    /// No earlier attempt ran under this identity; also every invocation that is not a mutation.
    First,
    /// An earlier attempt under the same `MutationId` ended without completing. Stores that
    /// create rows may already hold them and return the prior row for the repeated id.
    Resumed,
}

/// Authorization and mutation identity for one operation invocation.
#[derive(Debug, Clone)]
pub struct OperationContext {
    /// Authenticated caller; never taken from a request body.
    pub principal: Principal,
    /// Browser session the request arrived in; `None` for bearer and connector callers.
    pub session_id: Option<String>,
    /// Canonical operation being executed.
    pub operation: OperationName,
    /// Tenant grant when the request authorized a deployment target.
    pub tenant: Option<TenantGrant>,
    /// Workspace grants for every authorized workspace target, in target order.
    pub grants: Vec<WorkspaceGrant>,
    /// Durable write identity when this invocation is a mutation under an idempotency key.
    pub mutation: Option<MutationId>,
    /// Whether an earlier attempt already ran under `mutation`.
    pub attempt: Attempt,
}

impl WorkspaceGrant {
    /// Whether this grant includes `permission`.
    #[must_use]
    pub fn allows(&self, permission: Permission) -> bool {
        self.permissions.contains(&permission)
    }

    /// Workspace this grant covers.
    #[must_use]
    pub fn workspace_id(&self) -> WorkspaceId {
        self.scope.workspace_id
    }
}

impl TenantGrant {
    /// Whether this grant includes `permission`.
    #[must_use]
    pub fn allows(&self, permission: Permission) -> bool {
        self.permissions.contains(&permission)
    }
}

impl OperationContext {
    /// First workspace grant, when the operation named a workspace target.
    #[must_use]
    pub fn primary_workspace(&self) -> Option<&WorkspaceGrant> {
        self.grants.first()
    }
}
```

- [ ] **Step 5: Implement `mutations.rs`.** Replace lines 1–112 (from the first `//!` line through the closing brace of `pub trait AbandonedEffects`) with the block below. Leave `request_digest` (lines 114–133) exactly as it is; Task E.4 replaces it.

```rust
//! Durable mutation ledger: one write identity per (tenant, subject, operation, key).
//!
//! # Retention
//! Completed mutations are retained for 7 days; a key reused after that starts a new mutation.
//! A mutation that began and never completed stays until a later attempt completes it; the
//! 7-day TTL never drops it, or crash protection has a hole.
//!
//! # Resumed attempts
//! The ledger never looks into another store. Every store that creates a durable row takes the
//! `MutationId` and treats a repeated id as a no-op that returns the prior row. A handler is
//! therefore safe to run again under the same id: when `begin` reports `Abandoned`, dispatch
//! re-runs the handler with `Attempt::Resumed` and the same `MutationId`, and each store hands
//! back what the earlier attempt already wrote instead of writing it twice.

use okf_jawn_contract::error::ApiError;
use okf_jawn_contract::identity::{Digest, IdempotencyKey, MutationId, TenantId};
use okf_jawn_contract::metadata::OperationName;
use serde_json::Value;
use sha2::{Digest as ShaDigest, Sha256};

use crate::ports::PortFuture;

/// Ledger key: one caller can never read another caller's stored response.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct MutationKey {
    /// Tenant boundary.
    pub tenant_id: TenantId,
    /// Authenticated subject.
    pub subject: String,
    /// Canonical operation name.
    pub operation: OperationName,
    /// Caller-chosen retry identity.
    pub key: IdempotencyKey,
}

/// Atomic insert-or-read outcome for one begin attempt.
#[derive(Debug, Clone)]
pub enum BeginOutcome {
    /// No prior row; execute the handler under this identity.
    New(MutationId),
    /// Same key and digest already completed; return the stored response (or `AlreadyIssued`).
    Replay(StoredResponse),
    /// Same key, different digest.
    Conflict {
        /// Operation that first used the key.
        operation: OperationName,
    },
    /// Another attempt holds a live lease.
    InProgress {
        /// Mutation holding the lease.
        mutation_id: MutationId,
        /// Whole seconds before the caller should retry.
        retry_after: u32,
    },
    /// An earlier attempt ended without completing: its lease expired, or it was released
    /// after a handler error. The caller now holds the lease and re-runs the handler under
    /// the same identity.
    Abandoned {
        /// Mutation whose rows may already exist in the stores the handler writes.
        mutation_id: MutationId,
    },
}

/// Completed response body retained by the ledger.
#[derive(Debug, Clone)]
pub struct StoredResponse {
    /// Mutation that produced this response.
    pub mutation_id: MutationId,
    /// Serialized response body. Secret-bearing operations store only non-secret fields.
    pub body: Value,
}

/// Idempotency ledger owned by storage; adapters enforce uniqueness and leases.
pub trait MutationStore: Send + Sync {
    /// Atomic insert-or-read with a lease.
    ///
    /// `digest` is [`request_digest`] of the typed request.
    fn begin<'a>(
        &'a self,
        key: &'a MutationKey,
        digest: &'a Digest,
    ) -> PortFuture<'a, BeginOutcome>;

    /// Mark the mutation completed and retain the response for replay.
    fn complete(&self, mutation_id: MutationId, response: Value) -> PortFuture<'_, ()>;

    /// End the lease after a handler error. The row keeps its id; the same key may begin again.
    ///
    /// The next `begin` with the same key and digest returns `Abandoned` without waiting for
    /// the lease to expire. Releasing a completed mutation changes nothing.
    fn release(&self, mutation_id: MutationId) -> PortFuture<'_, ()>;
}
```

- [ ] **Step 6: Implement `dispatch.rs`.** Six edits, 6.1 to 6.6.

  **6.1** Lines 1–4, the `//!` header, become:

```rust
//! Route only declared operations through validation, target authorization, and the mutation ledger.
//!
//! Order: validate, decode, `targets()`, authorize every target, build the context,
//! `MutationStore::begin`, the handler, `complete`. The ledger never inspects other stores:
//! a resumed attempt re-runs the handler under the same `MutationId`.
```

  **6.2** Imports. Line 19 `use crate::context::{OperationContext, TenantGrant, WorkspaceGrant};` becomes `use crate::context::{Attempt, OperationContext, TenantGrant, WorkspaceGrant};`. Lines 20–22 (the `crate::mutations` import) become `use crate::mutations::{BeginOutcome, MutationKey, MutationStore, request_digest};`.

  **6.3** Delete `pub struct DispatchPorts` with its doc comment (lines 25–33) from above the macro. This also cures the ordering error at `:35` (a macro after a struct).

  **6.4** Replace the whole `macro_rules! dispatch_operations { … }` (lines 35–75) with:

```rust
macro_rules! dispatch_operations {
    ($(($id:ident, $request:ty, $response:ty, $path:literal, $label:literal, $alias:literal,
        $operator:literal, $visibility:literal, $permission:ident, $ui:literal, $status:literal,
        $destructive:literal, $description:literal)),* $(,)?) => {
        /// Dispatch one declared JSON operation into its typed implementation.
        ///
        /// # Errors
        /// Returns input, authorization, idempotency, implementation, or serialization errors.
        pub async fn dispatch(
            service: &dyn Application,
            ports: &DispatchPorts<'_>,
            caller: &Caller<'_>,
            operation_id: &str,
            input: Value,
        ) -> Result<Value, ApiError> {
            let operation = parse_operation(operation_id)?;
            match operation_id {
                $(stringify!($id) => {
                    static VALIDATOR: OnceLock<Result<jsonschema::Validator, String>> = OnceLock::new();
                    let request_value = input.clone();
                    let request: $request = decode_validated(input, &VALIDATOR)?;
                    let mut context =
                        authorize_targets(ports.access, caller, operation, &request).await?;
                    let replay = <$request as RequestScope>::REPLAY;
                    let key = request.idempotency_key().copied();
                    match prepare_mutation(ports, &mut context, replay, key, &request_value).await? {
                        MutationGate::Run => {
                            let response = service.$id(&context, request).await?;
                            finish_if_mutation(ports, &context, replay, response).await
                        }
                        MutationGate::ShortCircuit(value) => Ok(value),
                    }
                }),*
                _ => Err(ApiError::new(ErrorCode::NotFound, "Unknown operation")),
            }
        }
    };
}
```

  and insert directly after the macro, before `enum MutationGate`:

```rust
/// The authenticated caller of one dispatch.
#[derive(Debug, Clone, Copy)]
pub struct Caller<'a> {
    /// Server-established identity; never taken from a request body.
    pub principal: &'a Principal,
    /// Browser session; `None` for bearer and connector callers.
    pub session_id: Option<&'a str>,
}

/// Ports dispatch needs beyond the typed application handlers.
pub struct DispatchPorts<'a> {
    /// Grant lookup.
    pub access: &'a dyn AccessControl,
    /// Idempotency ledger.
    pub mutations: &'a dyn MutationStore,
}
```

  **6.5** Replace `async fn prepare_mutation` (lines 82–143) with:

```rust
async fn prepare_mutation(
    ports: &DispatchPorts<'_>,
    context: &mut OperationContext,
    replay: ReplayPolicy,
    idempotency_key: Option<IdempotencyKey>,
    request_value: &Value,
) -> Result<MutationGate, ApiError> {
    let Some(key) = idempotency_key else {
        return Ok(MutationGate::Run);
    };
    let digest = request_digest(request_value)?;
    let mutation_key = MutationKey {
        tenant_id: context.principal.tenant_id.clone(),
        subject: context.principal.subject.clone(),
        operation: context.operation,
        key,
    };
    match ports.mutations.begin(&mutation_key, &digest).await? {
        BeginOutcome::New(mutation_id) => {
            context.mutation = Some(mutation_id);
            Ok(MutationGate::Run)
        }
        BeginOutcome::Abandoned { mutation_id } => {
            context.mutation = Some(mutation_id);
            context.attempt = Attempt::Resumed;
            Ok(MutationGate::Run)
        }
        BeginOutcome::Replay(stored) => {
            replay_response(replay, stored.body).map(MutationGate::ShortCircuit)
        }
        BeginOutcome::Conflict { operation } => Err(ApiError::new(
            ErrorCode::Conflict,
            "Idempotency key was reused with a different request body",
        )
        .with_detail(ErrorDetail::IdempotencyConflict { operation })),
        BeginOutcome::InProgress {
            mutation_id,
            retry_after,
        } => Err(ApiError::new(
            ErrorCode::InProgress,
            "Another attempt with this idempotency key is still running",
        )
        .with_detail(ErrorDetail::InProgress {
            mutation_id,
            retry_after,
        })),
    }
}
```

  **6.6** Replace `async fn authorize_targets` (lines 208–243), whatever arm package C put there for `Target::Authenticated`, with:

```rust
async fn authorize_targets(
    access: &dyn AccessControl,
    caller: &Caller<'_>,
    operation: OperationName,
    request: &impl RequestScope,
) -> Result<OperationContext, ApiError> {
    let principal = caller.principal;
    let mut grants: Vec<WorkspaceGrant> = Vec::new();
    let mut tenant: Option<TenantGrant> = None;
    for target in request.targets() {
        match target {
            // Any signed-in principal; the handler filters its result by grants.
            Target::Authenticated => {}
            Target::Deployment(permission) => {
                let raw = access.authorize_tenant(principal, permission).await?;
                tenant = Some(access::authorize_tenant(principal, raw, permission)?);
            }
            Target::Workspace(workspace, permission) => {
                let raw = access.authorize(principal, workspace, permission).await?;
                let grant = access::authorize_workspace(principal, raw, permission)?;
                if !grants
                    .iter()
                    .any(|existing| existing.workspace_id() == workspace)
                {
                    grants.push(grant);
                }
            }
        }
    }
    Ok(OperationContext {
        principal: principal.clone(),
        session_id: caller.session_id.map(str::to_owned),
        operation,
        tenant,
        grants,
        mutation: None,
        attempt: Attempt::First,
    })
}
```

  Leave `finish_if_mutation`, `serialize_response`, `strip_connector_secret`, `replay_response`, `parse_operation`, `decode`, `decode_validated`, `new_mutation_id` and the `for_each_operation!` line as package C left them.

- [ ] **Step 7: Run and watch it pass.**

```powershell
cargo test --locked -p okf-jawn-contract -p okf-jawn-core 2>&1 | Select-String -Pattern '^test result|^error|FAILED|panicked'
```

  Expected: no `error`, no `FAILED`. The `dispatch` target reports `22 passed` (14 dispatch tests, 2 + 2 fixture self-tests, 4 `check::tests`); `authorization` reports `6 passed`.

- [ ] **Step 8: Format and commit.** Run the package format command (see Conventions), then:

```powershell
git add crates/core/src/context.rs crates/core/src/mutations.rs crates/core/src/dispatch.rs crates/core/tests/dispatch.rs crates/core/tests/support/mod.rs crates/core/tests/support/counting.rs
```

```text
feat(core): give dispatch a Caller, an Attempt and a ledger that resumes instead of looking up effects.

Why: Design section 3 (crash retry, tenant-level reads): a resumed attempt re-runs the handler under the same MutationId against stores that treat a repeated id as a no-op, so AbandonedEffects, record_effect and MutationStore::find are removed. Review findings: the in-progress test called the fixture store directly, the abandoned test used a lookup table, the delegation test was stopped by the route check, and the confirmation test exercised a store defined in the test file.
What changed: dispatch takes Caller { principal, session_id }; OperationContext carries session_id and attempt (First | Resumed); DispatchPorts loses effects; MutationStore is begin/complete/release; Target::Authenticated makes no adapter call; BeginOutcome::Abandoned sets attempt = Resumed and runs the handler. Fixtures and every dispatch test go through dispatch in the shared idiom; the confirmation re-consume test is deleted (storage gate confirmation-single-use owns it). crates/server no longer compiles on this branch until package H.
Verified: cargo test --locked -p okf-jawn-contract -p okf-jawn-core -> dispatch 22 passed, authorization 6 passed, 0 failed.
Next: Task E.3, release the lease when the handler fails.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_01LoZr4ME3crLYjcTNn5GcAC
```

### Task E.3: A handler error releases the lease

**Files:**
- Modify: `crates/core/src/dispatch.rs` — the `MutationGate::Run` arm of the macro (was `:64-67`) and `finish_if_mutation` (was `:145-161`)
- Modify: `crates/core/src/mutations.rs` — module docs (one paragraph)
- Test: `crates/core/tests/dispatch.rs`

**Interfaces:**
- Consumes: `MutationStore::release(&self, mutation_id: MutationId) -> PortFuture<'_, ()>` (Task E.2).
- Produces: private `async fn finish<Res: Serialize>(ports: &DispatchPorts<'_>, context: &OperationContext, _replay: ReplayPolicy, outcome: Result<Res, ApiError>) -> Result<Value, ApiError>` replacing `finish_if_mutation`. Rule: after `begin` returned `New` or `Abandoned`, a handler `Err` calls `release(mutation_id)` and returns the handler's error. If `release` itself fails, the handler's error is still what is returned and nothing is attached: the caller must act on the real failure, and an unreleased lease simply expires into `Abandoned`, which the next attempt resumes safely.

- [ ] **Step 1: Write the failing test.** In `crates/core/tests/dispatch.rs`, insert above the `#[path = …] mod check;` line at the end of the file:

```rust
#[tokio::test]
async fn handler_error_then_retry_runs_again() -> TestResult {
    let ports = ports_admin_a_read_b()?;
    let app = CountingApplication::new();
    app.set_response("create_item", item_document("hello"))?;
    app.fail_once(
        "create_item",
        ApiError::new(ErrorCode::Unavailable, "index offline"),
    )?;
    let alice = principal("alice", AccessRoute::LocalOwner)?;
    let body = create_item_body(WORKSPACE_A, KEY_ONE, "hello");
    let refused = err_of(call(&app, &ports, &alice, "create_item", body.clone()).await)?;
    assert_eq!(refused.code, ErrorCode::Unavailable);
    assert_eq!(refused.message, "index offline");

    let response = call(&app, &ports, &alice, "create_item", body).await?;
    assert_eq!(response.get("body"), Some(&json!("hello")));
    let seen = app.contexts("create_item")?;
    assert_eq!(seen.len(), 2);
    let earlier = some(seen.first(), "the failed attempt")?;
    let later = some(seen.get(1), "the retry")?;
    assert!(earlier.mutation.is_some());
    assert_eq!(later.mutation, earlier.mutation);
    assert_eq!(later.attempt, Attempt::Resumed);
    Ok(())
}
```

- [ ] **Step 2: Run and watch it fail.**

```powershell
cargo test --locked -p okf-jawn-core --test dispatch handler_error_then_retry_runs_again 2>&1 | Select-Object -Last 12
```

  Expected: `test handler_error_then_retry_runs_again ... FAILED` with `Error: ApiError { code: InProgress, message: "Another attempt with this idempotency key is still running"`.

- [ ] **Step 3: Implement.** In the macro, replace

```rust
                               let response = service.$id(&context, request).await?;
                               finish_if_mutation(ports, &context, replay, response).await
```

  with

```rust
                               let outcome = service.$id(&context, request).await;
                               finish(ports, &context, replay, outcome).await
```

  Replace `async fn finish_if_mutation` with:

```rust
/// Close the ledger row for a handler outcome and return what the caller receives.
async fn finish<Res: Serialize>(
    ports: &DispatchPorts<'_>,
    context: &OperationContext,
    _replay: ReplayPolicy,
    outcome: Result<Res, ApiError>,
) -> Result<Value, ApiError> {
    let response = match outcome {
        Ok(response) => response,
        Err(error) => {
            if let Some(mutation_id) = context.mutation {
                // The caller must see the handler's error. If the release itself fails, the
                // lease simply expires into `Abandoned`, which a later attempt resumes safely.
                let _released = ports.mutations.release(mutation_id).await;
            }
            return Err(error);
        }
    };
    let body = serialize_response(response)?;
    let Some(mutation_id) = context.mutation else {
        return Ok(body);
    };
    let stored = match context.operation {
        OperationName::CreateConnector => strip_connector_secret(body.clone())?,
        _ => body.clone(),
    };
    ports.mutations.complete(mutation_id, stored).await?;
    Ok(body)
}
```

  Replace the `//!` header of `crates/core/src/dispatch.rs` with:

```rust
//! Route only declared operations through validation, target authorization, and the mutation ledger.
//!
//! Order: validate, decode, `targets()`, authorize every target, build the context,
//! `MutationStore::begin`, the handler, then `complete` on success or `release` on error. The
//! ledger never inspects other stores: a resumed attempt re-runs the handler under the same
//! `MutationId`.
```

  In the `//!` header of `crates/core/src/mutations.rs`, append after the `# Resumed attempts` paragraph (directly before the blank line that precedes the imports):

```rust
//!
//! # Failed handlers
//! A handler error releases the lease. The row keeps its id and digest and is not completed,
//! so the same key and body may be sent again at once; that retry runs as a resumed attempt.
```

- [ ] **Step 4: Run and watch it pass.**

```powershell
cargo test --locked -p okf-jawn-core --test dispatch 2>&1 | Select-String -Pattern '^test result|FAILED'
```

  Expected: `test result: ok. 23 passed; 0 failed`.

- [ ] **Step 5: Format and commit.**

```powershell
git add crates/core/src/dispatch.rs crates/core/src/mutations.rs crates/core/tests/dispatch.rs
```

```text
fix(core): release the mutation lease when the handler fails.

Why: Design section 3 (failed write): a handler error releases the lease and the same key may be retried. dispatch left the handler through `?` and the trait had no release, so every failed write answered in_progress until the lease expired.
What changed: finish() replaces finish_if_mutation; on a handler Err it calls MutationStore::release and returns the handler's error unchanged (a failed release is ignored: the lease then expires into Abandoned, which resumes safely).
Verified: cargo test --locked -p okf-jawn-core --test dispatch -> 23 passed, 0 failed; handler_error_then_retry_runs_again failed with in_progress before the change.
Next: Task E.4, digest of the typed request with sorted keys.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_01LoZr4ME3crLYjcTNn5GcAC
```

### Task E.4: Request digest over the typed request with sorted keys

**Files:**
- Modify: `crates/core/Cargo.toml` — `[dev-dependencies]`
- Modify: `crates/core/src/mutations.rs` — imports (was `:13-19`) and `request_digest` (was `:114-133`)
- Modify: `crates/core/src/dispatch.rs` — two macro lines, `prepare_mutation`, one import
- Test: `crates/core/tests/dispatch.rs`

**Interfaces:**
- Consumes: `serde_json::Map<String, Value>: IntoIterator<Item = (String, Value)> + FromIterator<(String, Value)>` (`serde_json-1.0.151/src/map.rs:549`); `impl<T: ArrayLength<u8>> fmt::LowerHex for GenericArray<u8, T>` (`generic-array-0.14.7/src/hex.rs:27`), which is what `Sha256::digest` returns.
- Produces:

```rust
/// SHA-256 over the typed request re-serialized with object keys sorted; independent of
/// serde_json's `preserve_order` feature.
pub fn request_digest<T: serde::Serialize>(request: &T) -> Result<Digest, ApiError>;
```

  and private `async fn prepare_mutation<Req: RequestScope + Serialize>(ports: &DispatchPorts<'_>, context: &mut OperationContext, replay: ReplayPolicy, request: &Req) -> Result<MutationGate, ApiError>`.

  Why the test needs a manifest change: `serde_json::Map` is a `BTreeMap` unless `preserve_order` is on. `docling-core` turns it on in the real server graph; a core-only test build does not, so without the dev-dependency the reordered inputs would already be sorted and the test would prove nothing. `Cargo.lock` already lists `indexmap` under `serde_json`, so `--locked` still holds.

- [ ] **Step 1: Enable `preserve_order` for core's tests.** In `crates/core/Cargo.toml`, under `[dev-dependencies]`, add:

```toml
# Tests must see insertion-ordered JSON maps, as the real server graph does (docling-core).
serde_json = { workspace = true, features = ["preserve_order"] }
```

- [ ] **Step 2: Write the failing tests.** In `crates/core/tests/dispatch.rs` add `use okf_jawn_core::mutations::request_digest;` to the imports, and insert above the `mod check;` lines:

```rust
#[tokio::test]
async fn reordered_keys_replay_instead_of_conflicting() -> TestResult {
    let ports = ports_admin_a_read_b()?;
    let app = CountingApplication::new();
    app.set_response("create_item", item_document("hello"))?;
    let alice = principal("alice", AccessRoute::LocalOwner)?;
    let original = json!({
        "workspace_id": WORKSPACE_A,
        "base_revision": REVISION,
        "path": "notes/a.md",
        "title": "a",
        "type_name": "note",
        "kind": "note",
        "body": "hello",
        "properties": { "nested": { "alpha": 1, "beta": 2 } },
        "idempotency_key": KEY_ONE
    });
    let reordered = json!({
        "idempotency_key": KEY_ONE,
        "properties": { "nested": { "beta": 2, "alpha": 1 } },
        "body": "hello",
        "kind": "note",
        "type_name": "note",
        "title": "a",
        "path": "notes/a.md",
        "base_revision": REVISION,
        "workspace_id": WORKSPACE_A
    });
    // Precondition: `preserve_order` is enabled for this test build, so the two inputs are the
    // same JSON value written with different key order, at the top level and inside a value.
    assert_eq!(original, reordered);
    assert_ne!(
        serde_json::to_string(&original)?,
        serde_json::to_string(&reordered)?
    );

    let created = call(&app, &ports, &alice, "create_item", original).await?;
    let replayed = call(&app, &ports, &alice, "create_item", reordered).await?;
    assert_eq!(replayed, created);
    assert_eq!(app.call_count("create_item")?, 1);
    Ok(())
}

#[test]
fn request_digest_sorts_keys_at_every_depth() -> TestResult {
    let scrambled = json!({ "b": [{ "k": 1, "j": 2 }], "a": { "y": 2, "x": 1 } });
    // SHA-256 of the bytes `{"a":{"x":1,"y":2},"b":[{"j":2,"k":1}]}`.
    assert_eq!(
        request_digest(&scrambled)?.as_str(),
        "fa6628597d53c1e5019d96bfec141c0069e6959a5e0662f3899becda15925240"
    );
    Ok(())
}
```

- [ ] **Step 3: Run and watch them fail.**

```powershell
cargo test --locked -p okf-jawn-core --test dispatch 2>&1 | Select-String -Pattern 'FAILED|^Error|left:|right:|^test result'
git status --short Cargo.lock
```

  Expected: `reordered_keys_replay_instead_of_conflicting ... FAILED` with `Error: ApiError { code: Conflict, message: "Idempotency key was reused with a different request body"`; `request_digest_sorts_keys_at_every_depth ... FAILED` with a `left:`/`right:` digest mismatch; `Cargo.lock` unmodified (no output). If instead the first test fails on its `assert_ne!` precondition, Step 1 was not applied.

- [ ] **Step 4: Implement `mutations.rs`.** Replace the import block with:

```rust
use std::collections::BTreeMap;

use okf_jawn_contract::error::{ApiError, ErrorCode};
use okf_jawn_contract::identity::{Digest, IdempotencyKey, MutationId, TenantId};
use okf_jawn_contract::metadata::OperationName;
use serde::Serialize;
use serde_json::Value;
use sha2::{Digest as ShaDigest, Sha256};

use crate::ports::PortFuture;
```

  Replace `request_digest` (doc comment included) with:

```rust
/// SHA-256 over the typed request re-serialized with object keys sorted; independent of
/// `serde_json`'s `preserve_order` feature.
///
/// # Errors
/// Returns `Internal` when the request cannot be serialized.
pub fn request_digest<T: Serialize>(request: &T) -> Result<Digest, ApiError> {
    let value = serde_json::to_value(request).map_err(|error| internal(&error))?;
    let bytes = serde_json::to_vec(&canonical(value)).map_err(|error| internal(&error))?;
    let hash = Sha256::digest(bytes);
    Digest::try_from(format!("{hash:x}")).map_err(|error| internal(&error))
}

/// Rebuild `value` with the keys of every object in byte order, at every depth.
///
/// Collecting through a `BTreeMap` fixes the order whether `serde_json::Map` keeps insertion
/// order (`preserve_order`) or is itself a `BTreeMap`.
fn canonical(value: Value) -> Value {
    match value {
        Value::Object(map) => {
            let sorted: BTreeMap<String, Value> = map
                .into_iter()
                .map(|(key, child)| (key, canonical(child)))
                .collect();
            Value::Object(sorted.into_iter().collect())
        }
        Value::Array(items) => Value::Array(items.into_iter().map(canonical).collect()),
        scalar => scalar,
    }
}

fn internal(error: &dyn std::fmt::Display) -> ApiError {
    ApiError::new(ErrorCode::Internal, error.to_string())
}
```

  This also cures `clippy::format_collect` at `:126`.

- [ ] **Step 5: Implement `dispatch.rs`.** In the macro delete the lines `let request_value = input.clone();` and `let key = request.idempotency_key().copied();`, and change the `prepare_mutation` call to:

```rust
                       match prepare_mutation(ports, &mut context, replay, &request).await? {
```

  Replace `async fn prepare_mutation` with:

```rust
async fn prepare_mutation<Req: RequestScope + Serialize>(
    ports: &DispatchPorts<'_>,
    context: &mut OperationContext,
    replay: ReplayPolicy,
    request: &Req,
) -> Result<MutationGate, ApiError> {
    let Some(key) = request.idempotency_key().copied() else {
        return Ok(MutationGate::Run);
    };
    let digest = request_digest(request)?;
    let mutation_key = MutationKey {
        tenant_id: context.principal.tenant_id.clone(),
        subject: context.principal.subject.clone(),
        operation: context.operation,
        key,
    };
    match ports.mutations.begin(&mutation_key, &digest).await? {
        BeginOutcome::New(mutation_id) => {
            context.mutation = Some(mutation_id);
            Ok(MutationGate::Run)
        }
        BeginOutcome::Abandoned { mutation_id } => {
            context.mutation = Some(mutation_id);
            context.attempt = Attempt::Resumed;
            Ok(MutationGate::Run)
        }
        BeginOutcome::Replay(stored) => {
            replay_response(replay, stored.body).map(MutationGate::ShortCircuit)
        }
        BeginOutcome::Conflict { operation } => Err(ApiError::new(
            ErrorCode::Conflict,
            "Idempotency key was reused with a different request body",
        )
        .with_detail(ErrorDetail::IdempotencyConflict { operation })),
        BeginOutcome::InProgress {
            mutation_id,
            retry_after,
        } => Err(ApiError::new(
            ErrorCode::InProgress,
            "Another attempt with this idempotency key is still running",
        )
        .with_detail(ErrorDetail::InProgress {
            mutation_id,
            retry_after,
        })),
    }
}
```

  In the imports, `use okf_jawn_contract::identity::{ConnectorId, IdempotencyKey, MutationId};` → `use okf_jawn_contract::identity::{ConnectorId, MutationId};`.

- [ ] **Step 6: Run and watch them pass.**

```powershell
cargo test --locked -p okf-jawn-core --test dispatch 2>&1 | Select-String -Pattern '^test result|FAILED'
```

  Expected: `test result: ok. 25 passed; 0 failed`.

- [ ] **Step 7: Format and commit.**

```powershell
git add crates/core/Cargo.toml crates/core/src/mutations.rs crates/core/src/dispatch.rs crates/core/tests/dispatch.rs
```

```text
fix(core): digest the typed request with sorted keys.

Why: Design section 3 (digest): the idempotency digest must not depend on serde_json's preserve_order feature. dispatch hashed the raw input value, so with preserve_order on (docling-core enables it in the server graph) the same request with reordered keys was an idempotency conflict.
What changed: request_digest<T: Serialize> re-serializes the typed request and sorts object keys at every depth before hashing; dispatch passes the decoded request. core's dev-dependencies enable serde_json/preserve_order so the test sees what the server sees. Cargo.lock is unchanged.
Verified: cargo test --locked -p okf-jawn-core --test dispatch -> 25 passed, 0 failed; reordered_keys_replay_instead_of_conflicting failed with conflict before the change.
Next: Task E.5, decide the ledger body from ReplayPolicy.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_01LoZr4ME3crLYjcTNn5GcAC
```

### Task E.5: Ledger body decided by `ReplayPolicy`

**Files:**
- Modify: `crates/core/src/dispatch.rs` — `finish`, `serialize_response` (was `:163-166`), `strip_connector_secret` (was `:168-180`), `replay_response` (was `:182-206`), new `mod tests`
- Test: `crates/core/src/dispatch.rs` (`mod tests`); `crates/core/tests/dispatch.rs` (existing `create_connector_replay_is_already_issued_without_the_secret` and `a_resumed_create_connector_stores_only_the_connector_id` keep passing)

**Interfaces:**
- Consumes: `ReplayPolicy::{StoredResponse, AlreadyIssued { id_pointer: &'static str }}` (package C); `serde_json::Value::pointer(&self, pointer: &str) -> Option<&Value>` (`serde_json-1.0.151/src/value/mod.rs:779`).
- Produces: private `fn ledger_body(replay: ReplayPolicy, body: &Value) -> Result<Value, ApiError>`; `finish` takes `replay` (no longer `_replay`). `strip_connector_secret` and `serialize_response` are deleted. What the ledger stores no longer depends on `OperationName::CreateConnector`.

  This change is behaviour-preserving for the one operation that declares `AlreadyIssued`, so no test through `dispatch` can fail before it. The failing-first tests are unit tests of `ledger_body` with a pointer no operation uses (see Deviations 5).

- [ ] **Step 1: Write the failing tests.** Append to the end of `crates/core/src/dispatch.rs`, after the `okf_jawn_contract::for_each_operation!(dispatch_operations);` line:

```rust
#[cfg(test)]
mod tests {
    use okf_jawn_contract::error::{ApiError, ErrorCode, ErrorDetail};
    use okf_jawn_contract::scope::ReplayPolicy;
    use serde_json::json;

    use super::{ledger_body, replay_response};

    const CONNECTOR: &str = "cccccccc-cccc-4ccc-8ccc-cccccccccccc";
    const POLICY: ReplayPolicy = ReplayPolicy::AlreadyIssued {
        id_pointer: "/issued/id",
    };

    #[test]
    fn stored_response_policy_keeps_the_whole_body() -> Result<(), ApiError> {
        let body = json!({ "revision": "r", "warnings": [] });
        assert_eq!(ledger_body(ReplayPolicy::StoredResponse, &body)?, body);
        Ok(())
    }

    #[test]
    fn already_issued_policy_keeps_only_the_id_at_its_pointer() -> Result<(), ApiError> {
        let body = json!({ "issued": { "id": CONNECTOR, "label": "agent" }, "secret": "s" });
        assert_eq!(
            ledger_body(POLICY, &body)?,
            json!({ "connector_id": CONNECTOR })
        );
        Ok(())
    }

    #[test]
    fn already_issued_policy_refuses_a_response_without_the_id() {
        let refused = ledger_body(POLICY, &json!({ "secret": "s" }));
        assert!(matches!(refused, Err(error) if error.code == ErrorCode::Internal));
    }

    #[test]
    fn already_issued_replay_names_the_connector_and_returns_no_body() {
        let replayed = replay_response(POLICY, json!({ "connector_id": CONNECTOR }));
        assert!(matches!(
            replayed,
            Err(error) if error.code == ErrorCode::AlreadyIssued
                && matches!(
                    error.detail.as_deref(),
                    Some(ErrorDetail::AlreadyIssued { connector_id })
                        if connector_id.0.to_string() == CONNECTOR
                )
        ));
    }
}
```

- [ ] **Step 2: Run and watch it fail.**

```powershell
cargo test --locked -p okf-jawn-core --lib 2>&1 | Select-String -Pattern '^error' | Select-Object -First 5
```

  Expected: `error[E0432]`, unresolved import, naming `super::ledger_body`.

- [ ] **Step 3: Implement.** Replace `async fn finish` with:

```rust
/// Close the ledger row for a handler outcome and return what the caller receives.
async fn finish<Res: Serialize>(
    ports: &DispatchPorts<'_>,
    context: &OperationContext,
    replay: ReplayPolicy,
    outcome: Result<Res, ApiError>,
) -> Result<Value, ApiError> {
    let response = match outcome {
        Ok(response) => response,
        Err(error) => {
            if let Some(mutation_id) = context.mutation {
                // The caller must see the handler's error. If the release itself fails, the
                // lease simply expires into `Abandoned`, which a later attempt resumes safely.
                let _released = ports.mutations.release(mutation_id).await;
            }
            return Err(error);
        }
    };
    let body = serde_json::to_value(response)
        .map_err(|_| ApiError::new(ErrorCode::Internal, "Response serialization failed"))?;
    let Some(mutation_id) = context.mutation else {
        return Ok(body);
    };
    ports
        .mutations
        .complete(mutation_id, ledger_body(replay, &body)?)
        .await?;
    Ok(body)
}
```

  Add directly below it:

```rust
/// What the ledger retains for a completed mutation, decided by its replay policy.
fn ledger_body(replay: ReplayPolicy, body: &Value) -> Result<Value, ApiError> {
    match replay {
        ReplayPolicy::StoredResponse => Ok(body.clone()),
        ReplayPolicy::AlreadyIssued { id_pointer } => {
            let connector_id = body.pointer(id_pointer).ok_or_else(|| {
                ApiError::new(
                    ErrorCode::Internal,
                    "Issued response is missing the identity its replay policy names",
                )
            })?;
            Ok(serde_json::json!({ "connector_id": connector_id }))
        }
    }
}
```

  Delete `fn serialize_response` and `fn strip_connector_secret` (the latter was the `needless_pass_by_value` error at `:168`). Make `fn replay_response` read exactly:

```rust
fn replay_response(replay: ReplayPolicy, body: Value) -> Result<Value, ApiError> {
    match replay {
        ReplayPolicy::StoredResponse => Ok(body),
        ReplayPolicy::AlreadyIssued { .. } => {
            let connector_id: ConnectorId = body
                .get("connector_id")
                .cloned()
                .ok_or_else(|| {
                    ApiError::new(
                        ErrorCode::Internal,
                        "AlreadyIssued ledger row missing connector_id",
                    )
                })
                .and_then(|value| {
                    serde_json::from_value(value)
                        .map_err(|error| ApiError::new(ErrorCode::Internal, error.to_string()))
                })?;
            Err(ApiError::new(
                ErrorCode::AlreadyIssued,
                "Connector was already issued under this idempotency key",
            )
            .with_detail(ErrorDetail::AlreadyIssued { connector_id }))
        }
    }
}
```

- [ ] **Step 4: Run and watch it pass.**

```powershell
cargo test --locked -p okf-jawn-core 2>&1 | Select-String -Pattern '^test result|FAILED|^error'
```

  Expected: the lib target `4 passed`, `dispatch` `25 passed`, `authorization` `6 passed`, no `FAILED`.

- [ ] **Step 5: Format and commit.**

```powershell
git add crates/core/src/dispatch.rs
```

```text
refactor(core): decide the stored ledger body from ReplayPolicy.

Why: Review finding: finish_if_mutation chose what to store by OperationName::CreateConnector and carried a dead _replay parameter, so a second secret-bearing operation would have stored its secret. The contract already declares the rule per request (ReplayPolicy::AlreadyIssued { id_pointer }).
What changed: ledger_body(replay, &body) stores the whole body for StoredResponse and only {"connector_id": <value at id_pointer>} for AlreadyIssued; strip_connector_secret and serialize_response are gone. Every path that completes a mutation, first or resumed, goes through it.
Verified: cargo test --locked -p okf-jawn-core -> lib 4 passed, dispatch 25 passed, authorization 6 passed, 0 failed.
Next: Task E.6, refuse a grant for a different scope.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_01LoZr4ME3crLYjcTNn5GcAC
```

### Task E.6: A returned grant must be for the workspace and tenant asked for

**Files:**
- Modify: `crates/core/src/dispatch.rs` — `authorize_targets`, three new private functions, one import
- Test: `crates/core/tests/dispatch.rs`

**Interfaces:**
- Consumes: `WorkspaceGrant::workspace_id(&self) -> WorkspaceId`, `WorkspaceGrant.scope.tenant_id`, `TenantGrant.tenant_id`, `Principal.tenant_id`; fixture hooks `FixtureAccess::{answer_workspaces_as, answer_tenants_as}`.
- Produces: private `fn require_workspace_scope(principal: &Principal, workspace: WorkspaceId, grant: &WorkspaceGrant) -> Result<(), ApiError>`, `fn require_tenant_scope(principal: &Principal, grant: &TenantGrant) -> Result<(), ApiError>`, `fn foreign_scope() -> ApiError`. A mismatch returns `ErrorCode::Internal`, message `Access adapter returned a grant for a different scope`, before the handler and before the ledger.

- [ ] **Step 1: Write the failing tests.** In `crates/core/tests/dispatch.rs` add `use okf_jawn_core::storage::StorageScope;` to the imports, and insert above the `mod check;` lines:

```rust
#[tokio::test]
async fn a_grant_for_another_workspace_is_refused_before_the_handler() -> TestResult {
    let ports = ports_admin_a_read_b()?;
    ports.access.answer_workspaces_as(StorageScope {
        tenant_id: tenant("tenant-local")?,
        workspace_id: workspace(WORKSPACE_C)?,
    })?;
    let app = CountingApplication::new();
    app.set_response("list_items", listing())?;
    let alice = principal("alice", AccessRoute::LocalOwner)?;
    let input = list_items_body(WORKSPACE_A);
    let refused = err_of(call(&app, &ports, &alice, "list_items", input).await)?;
    assert_eq!(refused.code, ErrorCode::Internal);
    assert_eq!(
        refused.message,
        "Access adapter returned a grant for a different scope"
    );
    assert_eq!(app.call_count("list_items")?, 0);
    Ok(())
}

#[tokio::test]
async fn a_workspace_grant_for_another_tenant_is_refused_before_the_handler() -> TestResult {
    let ports = ports_admin_a_read_b()?;
    ports.access.answer_workspaces_as(StorageScope {
        tenant_id: tenant("tenant-other")?,
        workspace_id: workspace(WORKSPACE_A)?,
    })?;
    let app = CountingApplication::new();
    app.set_response("list_items", listing())?;
    let alice = principal("alice", AccessRoute::LocalOwner)?;
    let input = list_items_body(WORKSPACE_A);
    let refused = err_of(call(&app, &ports, &alice, "list_items", input).await)?;
    assert_eq!(refused.code, ErrorCode::Internal);
    assert_eq!(app.call_count("list_items")?, 0);
    Ok(())
}

#[tokio::test]
async fn a_tenant_grant_for_another_tenant_is_refused_before_the_handler() -> TestResult {
    let ports = ports_admin_a_read_b()?;
    ports.access.answer_tenants_as(tenant("tenant-other")?)?;
    let app = CountingApplication::new();
    app.set_response("create_workspace", created_workspace())?;
    let alice = principal("alice", AccessRoute::LocalOwner)?;
    let input = create_workspace_body(KEY_ONE);
    let refused = err_of(call(&app, &ports, &alice, "create_workspace", input).await)?;
    assert_eq!(refused.code, ErrorCode::Internal);
    assert_eq!(app.call_count("create_workspace")?, 0);
    Ok(())
}
```

- [ ] **Step 2: Run and watch them fail.**

```powershell
cargo test --locked -p okf-jawn-core --test dispatch is_refused_before_the_handler 2>&1 | Select-String -Pattern 'FAILED|^Error|^test result'
```

  Expected: all three `... FAILED`, each with `Error: "expected an error, got Ok(Object {`.

- [ ] **Step 3: Implement.** In the imports, `use okf_jawn_contract::identity::{ConnectorId, MutationId};` → `use okf_jawn_contract::identity::{ConnectorId, MutationId, WorkspaceId};`. Replace `async fn authorize_targets` with:

```rust
async fn authorize_targets(
    access: &dyn AccessControl,
    caller: &Caller<'_>,
    operation: OperationName,
    request: &impl RequestScope,
) -> Result<OperationContext, ApiError> {
    let principal = caller.principal;
    let mut grants: Vec<WorkspaceGrant> = Vec::new();
    let mut tenant: Option<TenantGrant> = None;
    for target in request.targets() {
        match target {
            // Any signed-in principal; the handler filters its result by grants.
            Target::Authenticated => {}
            Target::Deployment(permission) => {
                let raw = access.authorize_tenant(principal, permission).await?;
                require_tenant_scope(principal, &raw)?;
                tenant = Some(access::authorize_tenant(principal, raw, permission)?);
            }
            Target::Workspace(workspace, permission) => {
                let raw = access.authorize(principal, workspace, permission).await?;
                require_workspace_scope(principal, workspace, &raw)?;
                let grant = access::authorize_workspace(principal, raw, permission)?;
                if !grants
                    .iter()
                    .any(|existing| existing.workspace_id() == workspace)
                {
                    grants.push(grant);
                }
            }
        }
    }
    Ok(OperationContext {
        principal: principal.clone(),
        session_id: caller.session_id.map(str::to_owned),
        operation,
        tenant,
        grants,
        mutation: None,
        attempt: Attempt::First,
    })
}
```

  Add directly below it:

```rust
/// Refuse a workspace grant the adapter returned for a workspace or tenant that was not asked.
fn require_workspace_scope(
    principal: &Principal,
    workspace: WorkspaceId,
    grant: &WorkspaceGrant,
) -> Result<(), ApiError> {
    if grant.workspace_id() == workspace && grant.scope.tenant_id == principal.tenant_id {
        Ok(())
    } else {
        Err(foreign_scope())
    }
}

/// Refuse a tenant grant the adapter returned for a tenant other than the caller's.
fn require_tenant_scope(principal: &Principal, grant: &TenantGrant) -> Result<(), ApiError> {
    if grant.tenant_id == principal.tenant_id {
        Ok(())
    } else {
        Err(foreign_scope())
    }
}

fn foreign_scope() -> ApiError {
    ApiError::new(
        ErrorCode::Internal,
        "Access adapter returned a grant for a different scope",
    )
}
```

- [ ] **Step 4: Run and watch them pass.**

```powershell
cargo test --locked -p okf-jawn-core --test dispatch 2>&1 | Select-String -Pattern '^test result|FAILED'
```

  Expected: `test result: ok. 28 passed; 0 failed`.

- [ ] **Step 5: Format and commit.**

```powershell
git add crates/core/src/dispatch.rs crates/core/tests/dispatch.rs
```

```text
fix(core): refuse a grant the access adapter returned for a different scope.

Why: SPEC section 11: effective permissions come from AccessControl as WorkspaceGrant and TenantGrant. dispatch trusted whatever the adapter returned: a grant for another workspace or tenant became the handler's storage scope and also decided the delegation ceiling's workspace check.
What changed: after authorize / authorize_tenant, dispatch requires grant.workspace_id() == the workspace asked, grant.scope.tenant_id == principal.tenant_id and, for tenant grants, grant.tenant_id == principal.tenant_id; otherwise it returns Internal "Access adapter returned a grant for a different scope" without calling the handler.
Verified: cargo test --locked -p okf-jawn-core --test dispatch -> 28 passed, 0 failed; the three *_is_refused_before_the_handler tests failed with "expected an error, got Ok" before the change.
Next: Task E.7, name the offending field on schema failure.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_01LoZr4ME3crLYjcTNn5GcAC
```

### Task E.7: `ApiError.field` from the validator's instance path

**Files:**
- Modify: `crates/core/src/dispatch.rs` — `decode_validated` (was `:258-276`), new `invalid_input`, one import
- Test: `crates/core/tests/dispatch.rs`

**Interfaces:**
- Consumes (jsonschema 0.58.5, read from `~/.cargo/registry/src/index.crates.io-*/jsonschema-0.58.5/src/`):

```rust
// validator.rs:549
pub fn validate<'i>(&self, instance: F::Node<'i>) -> Result<(), ValidationError<'i>>;
// error.rs:440, :447
pub fn kind(&self) -> &ValidationErrorKind;
/// Returns the JSON Pointer to the instance location that failed validation.
pub fn instance_path(&self) -> &Location;
// error.rs:329 (variant of `pub enum ValidationErrorKind`, reachable as jsonschema::error::ValidationErrorKind)
Required { property: Value },
// paths.rs:690, :725, :730, :598
pub fn join<'a>(&self, segment: impl Into<LocationSegment<'a>>) -> Self;
pub fn as_str(&self) -> &str;
pub fn is_empty(&self) -> bool;
impl<'a> From<&'a String> for LocationSegment<'a>
```

  and `ApiError::with_field(self, field: impl Into<String>) -> Self` (package C).
- Produces: private `fn invalid_input(error: &jsonschema::ValidationError<'_>) -> ApiError`. `field` is a JSON Pointer into the request body (`/page/limit`). A missing required property is reported by the validator at the parent object, so its name is appended (`/idempotency_key`). A failure at the document root leaves `field` unset.

- [ ] **Step 1: Write the failing test.** In `crates/core/tests/dispatch.rs`, insert above the `mod check;` lines:

```rust
#[tokio::test]
async fn a_schema_failure_names_the_offending_field() -> TestResult {
    let ports = ports_admin_a_read_b()?;
    let app = CountingApplication::new();
    let alice = principal("alice", AccessRoute::LocalOwner)?;

    let mistyped = json!({
        "workspace_id": WORKSPACE_A,
        "at": { "kind": "latest" },
        "folder": "",
        "page": { "limit": "ten" }
    });
    let refused = err_of(call(&app, &ports, &alice, "list_items", mistyped).await)?;
    assert_eq!(refused.code, ErrorCode::InvalidInput);
    assert_eq!(refused.field.as_deref(), Some("/page/limit"));

    let keyless = json!({ "name": "n", "description": "d" });
    let refused = err_of(call(&app, &ports, &alice, "create_workspace", keyless).await)?;
    assert_eq!(refused.code, ErrorCode::InvalidInput);
    assert_eq!(refused.field.as_deref(), Some("/idempotency_key"));
    assert_eq!(app.call_count("list_items")?, 0);
    Ok(())
}
```

- [ ] **Step 2: Run and watch it fail.**

```powershell
cargo test --locked -p okf-jawn-core --test dispatch a_schema_failure_names_the_offending_field 2>&1 | Select-String -Pattern 'FAILED|left:|right:'
```

  Expected: `FAILED`, `left: None`, `right: Some("/page/limit")`.

- [ ] **Step 3: Implement.** Add `use jsonschema::error::ValidationErrorKind;` as the first line of the first import group. Replace `fn decode_validated` with the two functions:

```rust
fn decode_validated<T: DeserializeOwned + JsonSchema>(
    value: Value,
    cell: &OnceLock<Result<jsonschema::Validator, String>>,
) -> Result<T, ApiError> {
    let validator = cell
        .get_or_init(|| {
            let schema = SchemaSettings::draft2020_12()
                .into_generator()
                .into_root_schema_for::<T>();
            let document = serde_json::to_value(schema).map_err(|error| error.to_string())?;
            jsonschema::validator_for(&document).map_err(|error| error.to_string())
        })
        .as_ref()
        .map_err(|message| ApiError::new(ErrorCode::Internal, message.clone()))?;
    validator
        .validate(&value)
        .map_err(|error| invalid_input(&error))?;
    decode(value)
}

/// Report a schema failure and name the offending field as a JSON Pointer into the request.
///
/// A missing required property is reported at the parent object, so its name is appended.
fn invalid_input(error: &jsonschema::ValidationError<'_>) -> ApiError {
    let location = if let ValidationErrorKind::Required {
        property: Value::String(name),
    } = error.kind()
    {
        error.instance_path().join(name)
    } else {
        error.instance_path().clone()
    };
    let refused = ApiError::new(ErrorCode::InvalidInput, error.to_string());
    if location.is_empty() {
        refused
    } else {
        refused.with_field(location.as_str())
    }
}
```

- [ ] **Step 4: Run and watch it pass.**

```powershell
cargo test --locked -p okf-jawn-core --test dispatch 2>&1 | Select-String -Pattern '^test result|FAILED'
```

  Expected: `test result: ok. 29 passed; 0 failed`.

- [ ] **Step 5: Format and commit.**

```powershell
git add crates/core/src/dispatch.rs crates/core/tests/dispatch.rs
```

```text
feat(core): name the offending field when schema validation fails.

Why: ApiError.field ("Input field needing correction") was never set by dispatch, so a client had to parse the validator's message.
What changed: a schema failure sets ApiError.field to the validator's instance path as a JSON Pointer; for a missing required property the property name is appended to the parent path the validator reports.
Verified: cargo test --locked -p okf-jawn-core --test dispatch -> 29 passed, 0 failed; a_schema_failure_names_the_offending_field failed with left: None before the change.
Next: Task E.8, remaining lint debt and the package gate.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_01LoZr4ME3crLYjcTNn5GcAC
```

### Task E.8: Remaining lint debt, final module docs, package gate

**Files:**
- Modify: `crates/core/src/access.rs:109-144` (move one item)
- Modify: `crates/core/src/dispatch.rs:1-5` (module docs)
- Test: the package gate

**Interfaces:**
- Consumes / Produces: no signature changes.

Every lint error CI run 37352882339 reported in this package's four source files, and where it is cured:

| Site (b205c4a) | Lint | Fix | Task |
| --- | --- | --- | --- |
| `access.rs:113` (trait after `fn authorize_tenant` at `:93`) | `arbitrary_source_item_ordering` | move `pub trait AccessControl` above the functions | E.8 Step 1 |
| `dispatch.rs:35` (macro after `struct DispatchPorts` at `:26`) | `arbitrary_source_item_ordering` | types now follow the macro | E.2 |
| `dispatch.rs:168:33` `strip_connector_secret(body: Value)` | `needless_pass_by_value` | function deleted; `ledger_body(replay, &body)` | E.5 |
| `mutations.rs:39:79` `AlreadyIssued` in a doc comment | `doc_markdown` | backticks | E.2 |
| `mutations.rs:81`, `:95` `record_effect<'a>`, `find<'a>` | `elidable_lifetime_names` | methods deleted | E.2 |
| `mutations.rs:88` `complete<'a>(&'a self, …)` | `elidable_lifetime_names` | `fn complete(&self, …) -> PortFuture<'_, ()>` | E.2 |
| `mutations.rs:103:22`, `:103:51`, `:107` (`AbandonedEffects`) | `doc_markdown` ×2, `elidable_lifetime_names` | trait deleted | E.2 |
| `mutations.rs:126:23` (`format!` inside `.map(..).collect()`) | `format_collect` | `format!("{hash:x}")` | E.4 |
| `access.rs:21,77,97`, `dispatch.rs:48,88,150,163,168,182,195,213,245,253,261`, `mutations.rs:118` | `result_large_err` | boxed `ApiError.detail` (package C); nothing to do here | C |
| `context.rs` | none reported | — | — |

- [ ] **Step 1: Move the trait in `access.rs`.** Cut the whole `pub trait AccessControl: Send + Sync { … }` item with its doc comment (lines 109–144, from `/// Resolves effective workspace and tenant grants for an authenticated principal.` through its closing `}`) and paste it directly after the imports, above the doc comment of `pub fn check_route` (line 17). Nothing else in the file changes.

- [ ] **Step 2: Final module docs of `dispatch.rs`.** Replace the `//!` header with:

```rust
//! Route only declared operations through validation, target authorization, and the mutation ledger.
//!
//! Order: validate, decode, `targets()`, authorize every target, build the context,
//! `MutationStore::begin`, the handler, then `complete` on success or `release` on error.
//! A grant is used only when it is for the workspace and tenant that were asked for. The ledger
//! never inspects other stores: a resumed attempt re-runs the handler under the same
//! `MutationId`, and what the ledger retains is decided by the request's `ReplayPolicy`.
```

- [ ] **Step 3: Check the final shape of `dispatch.rs`.** Top-level items, in this order: the imports; `macro_rules! dispatch_operations`; `Caller`; `DispatchPorts`; `MutationGate`; `prepare_mutation`; `finish`; `ledger_body`; `replay_response`; `authorize_targets`; `require_workspace_scope`; `require_tenant_scope`; `foreign_scope`; `parse_operation`; `decode`; `decode_validated`; `invalid_input`; `new_mutation_id`; `okf_jawn_contract::for_each_operation!(dispatch_operations);`; `#[cfg(test)] mod tests`. The imports read exactly:

```rust
use jsonschema::error::ValidationErrorKind;
use okf_jawn_contract::access::Principal;
use okf_jawn_contract::error::{ApiError, ErrorCode, ErrorDetail};
use okf_jawn_contract::identity::{ConnectorId, MutationId, WorkspaceId};
use okf_jawn_contract::metadata::OperationName;
use okf_jawn_contract::scope::{ReplayPolicy, RequestScope, Target};
use schemars::{JsonSchema, generate::SchemaSettings};
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::Value;
use std::sync::OnceLock;
use uuid::Uuid;

use crate::access::{self, AccessControl};
use crate::context::{Attempt, OperationContext, TenantGrant, WorkspaceGrant};
use crate::mutations::{BeginOutcome, MutationKey, MutationStore, request_digest};
use crate::ports::Application;
```

  and the macro reads exactly:

```rust
macro_rules! dispatch_operations {
    ($(($id:ident, $request:ty, $response:ty, $path:literal, $label:literal, $alias:literal,
        $operator:literal, $visibility:literal, $permission:ident, $ui:literal, $status:literal,
        $destructive:literal, $description:literal)),* $(,)?) => {
        /// Dispatch one declared JSON operation into its typed implementation.
        ///
        /// # Errors
        /// Returns input, authorization, idempotency, implementation, or serialization errors.
        pub async fn dispatch(
            service: &dyn Application,
            ports: &DispatchPorts<'_>,
            caller: &Caller<'_>,
            operation_id: &str,
            input: Value,
        ) -> Result<Value, ApiError> {
            let operation = parse_operation(operation_id)?;
            match operation_id {
                $(stringify!($id) => {
                    static VALIDATOR: OnceLock<Result<jsonschema::Validator, String>> = OnceLock::new();
                    let request: $request = decode_validated(input, &VALIDATOR)?;
                    let mut context =
                        authorize_targets(ports.access, caller, operation, &request).await?;
                    let replay = <$request as RequestScope>::REPLAY;
                    match prepare_mutation(ports, &mut context, replay, &request).await? {
                        MutationGate::Run => {
                            let outcome = service.$id(&context, request).await;
                            finish(ports, &context, replay, outcome).await
                        }
                        MutationGate::ShortCircuit(value) => Ok(value),
                    }
                }),*
                _ => Err(ApiError::new(ErrorCode::NotFound, "Unknown operation")),
            }
        }
    };
}
```

- [ ] **Step 4: Format check, scoped to this package.**

```powershell
rustfmt --edition 2024 crates\core\src\dispatch.rs crates\core\src\mutations.rs crates\core\src\context.rs crates\core\src\access.rs crates\core\tests\dispatch.rs crates\core\tests\authorization.rs
cargo fmt --all --check 2>&1 | Select-String -Pattern '^Diff in'
```

  Expected: no `Diff in` line names `crates\core\src\{dispatch,mutations,context,access}.rs`, `crates\core\tests\…` or `tests\support\check.rs`. `Diff in` lines for other files are not yours — list them in the handoff, do not touch them. Package C's acceptance names the ones left after C: `crates\core\src\drafts.rs` (F), `crates\server\tests\bindings.rs` (H), `xtask\src\api.rs` (D), `qualification\docling\src\main.rs` (B).

- [ ] **Step 5: Clippy — the gate, then the scoped judgement.** The gate command:

```powershell
New-Item -ItemType Directory -Force .artifacts | Out-Null
cargo clippy --locked -p okf-jawn-contract -p okf-jawn-core --all-targets --message-format=short -- -D warnings 2>&1 | Tee-Object .artifacts\cure-dispatch-clippy-gate.log | Select-String -Pattern 'error|warning: unused|could not compile'
```

  If it ends without `error`, the gate is green; go to Step 6. Otherwise read where the errors are:

```powershell
Select-String -Path .artifacts\cure-dispatch-clippy-gate.log -Pattern 'crates[\\/]core[\\/]src[\\/](dispatch|mutations|context|access)\.rs', 'crates[\\/]core[\\/]tests[\\/]', 'tests[\\/]support[\\/]'
```

  Any line here is your defect: fix it (rename, restructure, backtick the identifier — never `#[allow]`, never `#[expect]`) and repeat. Errors only in other core source files (expected until package F merges: `crates\core\src\storage.rs:77` item ordering, `crates\core\src\credentials.rs:41` elidable lifetime, and whatever follows them) are package F's. But they stop Clippy at the library, so it never analyses `crates/core/tests`. To judge your test files anyway, list all diagnostics once with lint levels capped to warnings. This is a way to **read** diagnostics past another package's errors; it is not a gate, proves nothing by exiting 0, and must never appear in a script, config or commit (SPEC §14 forbids cap-lints as a workaround):

```powershell
cargo clippy --locked -p okf-jawn-contract -p okf-jawn-core --all-targets --message-format=short -- --cap-lints warn 2>&1 | Tee-Object .artifacts\cure-dispatch-clippy-all.log | Out-Null
Select-String -Path .artifacts\cure-dispatch-clippy-all.log -Pattern 'crates[\\/]core[\\/]src[\\/](dispatch|mutations|context|access)\.rs', 'crates[\\/]core[\\/]tests[\\/]', 'tests[\\/]support[\\/]'
```

  Expected: no output — zero diagnostics (Clippy lints, `dead_code`, unused imports) in the four source files, in all of `crates/core/tests`, and in `tests/support/check.rs`. What you must clear is exactly that set of files; what you must not clear is anything else.

- [ ] **Step 6: Tests.**

```powershell
cargo test --locked -p okf-jawn-contract -p okf-jawn-core 2>&1 | Select-String -Pattern '^test result|FAILED|^error'
```

  Expected: no `FAILED`; core lib `4 passed`, `authorization` `6 passed`, `dispatch` `29 passed`; the contract targets pass as they did at the branch point.

- [ ] **Step 7: Commit.**

```powershell
git add crates/core/src/access.rs crates/core/src/dispatch.rs
```

```text
refactor(core): order access.rs items and finish the dispatch module docs.

Why: CI run 37352882339 failed Clippy in okf-jawn-core; of the errors in dispatch, mutations, context and access, the trait-after-functions ordering in access.rs:113 was the last one left after tasks E.2 to E.7 (SPEC section 14: types before behavior).
What changed: AccessControl is declared before the functions in access.rs; the dispatch module docs state the final order and rules. No behaviour change.
Verified: cargo test --locked -p okf-jawn-contract -p okf-jawn-core -> lib 4, authorization 6, dispatch 29 passed, 0 failed. Clippy (--all-targets, -D warnings) reports no diagnostic in crates/core/src/{dispatch,mutations,context,access}.rs, crates/core/tests or tests/support/check.rs. GATE-SENTENCE
Next: Blocked: crates/server does not compile against the new dispatch signature until package H; the orchestrator re-runs the uncapped Clippy gate after package F merges.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_01LoZr4ME3crLYjcTNn5GcAC
```

  Replace `GATE-SENTENCE` with what Step 5 showed: either `The package gate exits 0.` or `The gate still fails only in <the files Step 5 listed>, which belong to package F.` with the real file names.

- [ ] **Step 8: Handoff.** Report: the eight commits; gate results as observed; the files outside this package named by `cargo fmt --all --check` and by the Clippy gate; and the notes for the orchestrator in Deviations 2, 4, 6, 9, 10 and 13.

### Sites removed for the test lints (requirement 9)

All line numbers are `b205c4a`. 21 `unwrap`/`expect`/`expect_err`/`panic!` sites and 2 `[]` index sites; every one is gone after Tasks E.1 and E.2.

| Site | Was | Now |
| --- | --- | --- |
| `tests/authorization.rs:13` | `.expect("tenant")` | `local_tenant()?`; `grant` returns `Result` |
| `tests/dispatch.rs:149` | `.expect_err("write in B must fail")` | `err_of(call(…).await)?` in `write_needs_a_write_grant_and_read_needs_only_read` |
| `tests/dispatch.rs:225` | `.expect_err("foreign binding must fail")` | `err_of(…)?` in `present_with_a_foreign_binding_is_forbidden_before_the_handler` |
| `tests/dispatch.rs:266` | `.expect_err("missing tenant grant")` | `err_of(…)?` in `create_workspace_without_a_tenant_grant_is_forbidden` |
| `tests/dispatch.rs:294` | `.expect_err("delegated read cannot write")` | test rewritten as `delegation_ceiling_decides_whether_a_connector_may_propose` (`err_of`) |
| `tests/dispatch.rs:333` | `other["body"] = json!("different")` | `create_item_body(WORKSPACE_A, KEY_ONE, "different")` |
| `tests/dispatch.rs:336` | `.expect_err("digest conflict")` | `err_of(…)?` in `the_same_key_with_a_different_body_conflicts` |
| `tests/dispatch.rs:385` | `panic!("expected New")` | test rewritten as `an_abandoned_attempt_reruns_the_handler_once_as_resumed`; no direct `begin` |
| `tests/dispatch.rs:430` | `panic!("expected New")` | test rewritten as `a_resumed_create_connector_stores_only_the_connector_id` |
| `tests/dispatch.rs:448`, `:451`, `:456` | `.expect_err`, `.expect("AlreadyIssued detail")`, `panic!` | deleted with that test; `create_connector_replay_is_already_issued_without_the_secret` asserts the same through `err_of`, `some`, `matches!` |
| `tests/dispatch.rs:545` | `first["secret"]` | `issued.get("secret")` |
| `tests/dispatch.rs:548` | `.expect_err("replay must be AlreadyIssued")` | `err_of(…)?` |
| `tests/dispatch.rs:553` | `.lock().unwrap().expect("mutation")` (2 sites) | `some(…)?` over the mutation id recorded in `app.contexts("create_connector")` |
| `tests/dispatch.rs:555` | `.expect("ledger row")` | `ports.mutations.stored_body(mutation_id)?` compared with `Some(json!(…))` |
| `tests/dispatch.rs:610` | `.expect_err("different mutation cannot re-consume")` | deleted with the confirmation test and `FixtureConfirmationStore` (`:561-613`, `:642-730`) |
| `tests/dispatch.rs:636` | `.expect_err("missing idempotency_key")` | `err_of(…)?` in `a_write_without_an_idempotency_key_is_rejected_by_the_schema` |
| `tests/support/mod.rs:220` | `.expect("key present")` in `force_abandon` | `force_abandon` deleted; `expire_leases` and `release` |
| `tests/support/mod.rs:542`, `:548`, `:554` | `.expect(…)` in `workspace`, `tenant`, `idempotency_key` | the helpers return `Result` |

Other denied lints cured in the same files while rewriting them: `tests/support/mod.rs:341` `as u32` (`cast_possible_truncation`) → `u32::try_from(…).unwrap_or(u32::MAX)`; `:292`, `:312`, `:352`, `:373` `now + self.lease` (`arithmetic_side_effects`) → `checked_add`; `tests/support/counting.rs:82` `+= 1` → the count is the number of recorded calls; `tests/dispatch.rs:3` and `tests/support/mod.rs:3` `mod` before `use` (`arbitrary_source_item_ordering`) → `mod` last.

### Package E acceptance

A verifying agent that did not write the package runs, from `D:\okf\cure\dispatch` in PowerShell:

1. Scope: `git diff --name-only (git merge-base HEAD integration/foundation-cure) HEAD` lists exactly: `crates/core/Cargo.toml`, `crates/core/src/{access,context,dispatch,mutations}.rs`, `crates/core/tests/{authorization,dispatch}.rs`, `crates/core/tests/support/{mod,counting}.rs`, `tests/support/check.rs`. `git diff (git merge-base HEAD integration/foundation-cure) HEAD -- crates/core/Cargo.toml` adds only the `serde_json` line (and its comment) under `[dev-dependencies]`; `Cargo.lock` and `crates/core/src/lib.rs` are not in the list.
2. `cargo fmt --all --check 2>&1 | Select-String '^Diff in'` → no line for this package's files.
3. `cargo clippy --locked -p okf-jawn-contract -p okf-jawn-core --all-targets -- -D warnings` → exit 0 once package F is merged into the branch being verified; before that, no diagnostic in this package's files by the procedure of Task E.8 Step 5.
4. `cargo test --locked -p okf-jawn-contract -p okf-jawn-core` → 0 failed; core lib 4 passed, `authorization` 6 passed, `dispatch` 29 passed.
5. Policy text search (must print nothing):

```powershell
Select-String -Path crates\core\tests\*.rs, crates\core\tests\support\*.rs, tests\support\check.rs -Pattern '\.unwrap\(\)', '\.expect\(', 'expect_err', 'panic!', 'unreachable!', '#\[allow', '#\[expect', '\w\["'
Select-String -Path crates\core\src\*.rs, crates\core\tests\*.rs, crates\core\tests\support\*.rs -Pattern 'AbandonedEffects', 'record_effect', 'FixtureEffects', 'force_abandon', 'FixtureConfirmationStore'
Select-String -Path crates\core\tests\dispatch.rs -Pattern '\.begin\(', 'MutationKey'
```

6. Mutation checks. Apply one edit to product code, run the named command, see the named tests fail, then `git checkout -- crates/core/src`. Command unless stated: `cargo test --locked -p okf-jawn-core --test dispatch`.

   | Edit | Tests that must fail |
   | --- | --- |
   | `dispatch.rs`, `finish`: delete the line `let _released = ports.mutations.release(mutation_id).await;` | `handler_error_then_retry_runs_again` |
   | `mutations.rs`, `request_digest`: `serde_json::to_vec(&canonical(value))` → `serde_json::to_vec(&value)` | `reordered_keys_replay_instead_of_conflicting`, `request_digest_sorts_keys_at_every_depth` |
   | `dispatch.rs`, `prepare_mutation`: `request_digest(request)?` → `request_digest(&context.operation)?` | `the_same_key_with_a_different_body_conflicts` |
   | `dispatch.rs`, `ledger_body`: body of the `AlreadyIssued` arm → `Ok(body.clone())` (keep `id_pointer` bound as `id_pointer: _`) | `a_resumed_create_connector_stores_only_the_connector_id`, `create_connector_replay_is_already_issued_without_the_secret`; and with `cargo test --locked -p okf-jawn-core --lib`: `already_issued_policy_keeps_only_the_id_at_its_pointer`, `already_issued_policy_refuses_a_response_without_the_id` |
   | `dispatch.rs`, `prepare_mutation`: delete `context.attempt = Attempt::Resumed;` | `an_abandoned_attempt_reruns_the_handler_once_as_resumed`, `a_resumed_create_connector_stores_only_the_connector_id`, `handler_error_then_retry_runs_again` |
   | `dispatch.rs`, `prepare_mutation`: replace the whole `BeginOutcome::InProgress { … } => Err(…)` arm with `BeginOutcome::InProgress { .. } => Ok(MutationGate::Run),` | `a_second_attempt_during_a_live_lease_is_in_progress` |
   | `dispatch.rs`, `prepare_mutation`: `BeginOutcome::Replay(stored) => { … }` → `BeginOutcome::Replay(_) => Ok(MutationGate::Run),` | `replay_returns_the_stored_response_and_the_handler_runs_once`, `create_connector_replay_is_already_issued_without_the_secret` |
   | `dispatch.rs`, `require_workspace_scope`: delete `grant.workspace_id() == workspace && ` | `a_grant_for_another_workspace_is_refused_before_the_handler` |
   | `dispatch.rs`, `require_workspace_scope`: delete ` && grant.scope.tenant_id == principal.tenant_id` | `a_workspace_grant_for_another_tenant_is_refused_before_the_handler` |
   | `dispatch.rs`, `require_tenant_scope`: the `if` condition → `true` | `a_tenant_grant_for_another_tenant_is_refused_before_the_handler` |
   | `dispatch.rs`, `authorize_targets`: `Target::Authenticated => {}` → `Target::Authenticated => { access.authorize_tenant(principal, okf_jawn_contract::access::Permission::Read).await?; }` | `an_authenticated_target_needs_no_grant_and_no_adapter_lookup` |
   | `dispatch.rs`, `authorize_targets`: `session_id: caller.session_id.map(str::to_owned),` → `session_id: None,` | `the_session_id_of_the_caller_reaches_the_handler` |
   | `dispatch.rs`, `invalid_input`: `refused.with_field(location.as_str())` → `refused` | `a_schema_failure_names_the_offending_field` |
   | `access.rs`, `authorize_workspace`: delete the line `grant.permissions = apply_delegation(principal, Some(grant.workspace_id()), &grant.permissions);` | `delegation_ceiling_decides_whether_a_connector_may_propose`; and `cargo test --locked -p okf-jawn-core --test authorization`: `local_owner_uses_the_same_rules_and_cannot_reach_other_workspaces` |
   | `access.rs`, `authorize_workspace`: delete the `if !grant.allows(permission) { … }` block | `write_needs_a_write_grant_and_read_needs_only_read` |
   | `dispatch.rs`, `prepare_mutation`: `subject: context.principal.subject.clone(),` → `subject: String::new(),` | `the_same_key_from_two_subjects_is_two_mutations` |

### Deviations

1. **`complete` / `release` spelling.** The shared interfaces write `fn complete<'a>(&'a self, …) -> PortFuture<'a, ()>` and `fn release<'a>(&'a self, …) -> PortFuture<'a, ()>`. Clippy `elidable_lifetime_names` (pedantic, denied) rejects exactly that shape: CI run 37352882339 reports it at `crates/core/src/mutations.rs:81`, `:88`, `:95`, `:107`. This plan writes `fn complete(&self, …) -> PortFuture<'_, ()>` and the same for `release`. The type is identical; every implementer (fixtures, storage, package H's tests) must use the elided spelling too.
2. **The Clippy and fmt gates cannot be fully green on this branch alone.** At `b205c4a` Clippy also fails in `crates/core/src/storage.rs:77` and `crates/core/src/credentials.rs:41` (package F, in parallel), which stops it at the library before it analyses `crates/core/tests`; `cargo fmt --all --check` still names files of packages F, H, D and B (listed in Task E.8 Step 4). Task E.8 Step 5 therefore judges this package by path-filtered diagnostics, using a one-off `--cap-lints warn` listing only to read the test targets' diagnostics. The uncapped gate must be re-run by the orchestrator after F merges.
3. **Delegation ceiling test.** The brief says "use a Propose-capable ceiling against a Write request so the ceiling is what refuses". On the `McpDelegation` route that cannot isolate the ceiling: `check_route` (`crates/core/src/access.rs:22-23`) refuses every agent request other than Read or Propose before the ceiling is consulted, whatever the ceiling holds. The rewritten test isolates the ceiling the other way round — a Read-only ceiling against a Propose request (route check passes, raw grant includes Propose, only the ceiling can refuse), with the positive control that a Read+Propose ceiling succeeds — and keeps the brief's literal case (Propose-capable ceiling, Write request → Forbidden) as a third assertion. This matches the design's wording "delegation ceiling tested with Propose".
4. **What `begin` returns after `release`.** The shared interfaces say only "The row keeps its id; the same key may begin again." This plan fixes it in the trait docs and the fixture: the next `begin` with the same key and digest returns `Abandoned { mutation_id }` at once, so the retry runs with `Attempt::Resumed`. Reason: the failed handler may already have created rows under that id (for `create_connector`, a credential), and `Resumed` is how a handler learns that. The storage lane's real `MutationStore` must implement the same rule; `handler_error_then_retry_runs_again` asserts it.
5. **Ledger body by policy has no failing-first test through `dispatch`.** `create_connector` is the only operation declaring `AlreadyIssued`, and both the old name-based code and the new policy-based code store `{connector_id}` for it. The change is guarded by four unit tests of `ledger_body` / `replay_response` in `crates/core/src/dispatch.rs` (`mod tests`) using a pointer no operation declares; they fail to compile before the change and fail when the rule is removed. `a_resumed_create_connector_stores_only_the_connector_id` (Task E.2) is the through-`dispatch` regression guard.
6. **`field` for a missing required property** goes one step beyond "the validator's instance path": jsonschema reports `required` at the parent object, which for a top-level field is the empty path, so the property name is appended. Orchestrator: `ApiError.field`'s contract doc ("Input field needing correction") does not say it is a JSON Pointer; consider saying so in `crates/contract/src/error.rs`.
7. **Tests of Task E.2 fail first by compile error only.** The signature change makes the old and new test files mutually exclusive; the 14 dispatch tests and 4 fixture self-tests introduced there cannot be shown failing at run time first. Each has a mutation in the acceptance table instead. In particular, package C's Task C.5 already leaves `Target::Authenticated => {}` in `authorize_targets` as its minimum edit, so the Authenticated behaviour predates this package; `an_authenticated_target_needs_no_grant_and_no_adapter_lookup` guards it.
8. **Count of lint sites.** The design says "all 21 violations"; that is the number of `unwrap`/`expect`/`expect_err`/`panic!` sites. There are also 2 `[]` index sites on `serde_json::Value` (`tests/dispatch.rs:333`, `:545`), 23 in all, plus the arithmetic, cast and ordering errors listed under the table.
9. **Stale prose outside this package's files.** `crates/core/AGENTS.md` names "confirmation re-consume" in the lane receipt; that test is deleted here (the storage gate `confirmation-single-use` owns the rule). `crates/storage/AGENTS.md` may still mention `record_effect` / `find`. Orchestrator or package F to correct. Conversely, deleting `FixtureConfirmationStore` here removes the fixture that package F's plan says must lose its `find_by_mutation` method at integration (F's Deviations 8): after both merge there is nothing left to repair.
10. **Shared fixtures and `dead_code`.** `tests/support/check.rs` and `crates/core/tests/support/**` are `#[path]`-included by several test targets, each of which is its own crate; an item one target does not use is `dead_code` there, and suppression is forbidden. Hence the self-tests inside each support file and the rule that includers declare `mod check;` at the crate root. Package H depends on both.
11. **`Caller` derives `Debug, Clone, Copy`** — additive to the shared interface.
12. **Not compiled.** No cargo run was permitted while writing this plan and no build artifacts existed. Every listing was formatted with the repository's `rustfmt.toml` (rustfmt 1.99.0 toolchain) and `tests/support/check.rs` was compiled and run standalone with `rustc --test` (4 passed); nothing else has been type-checked. APIs of axum, jsonschema, serde_json, generic-array and tokio were read from the vendored sources cited in each task. A compile error on first build is the implementer's defect to fix inside the task (no suppression); a mismatch with package C's actual types is a stop-and-report.
13. **`crates/core/src/lib.rs` is left untouched.** Its line 28, `/// Durable mutation ledger and abandoned-effect lookup.`, is stale after Task E.2. Package F, in parallel, rewrites that same line to `/// Durable mutation ledger.` (its lib.rs task); editing it here too would be a same-line merge conflict. Until F merges, the doc line on this branch is stale; nothing else in the file concerns this package.
