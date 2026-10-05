## Shared interfaces (fixed before any package starts)

Every package's tasks use exactly these names and types. A package that finds one of them
cannot be implemented as written stops and reports to the orchestrator; it does not adapt.

### Operation table tuple (package C produces; every macro consumer matches it)

`crates/contract/src/operations.rs` rows become 13 fields, in this order:

```text
($id:ident, $request:ty, $response:ty, $path:literal, $label:literal, $alias:literal,
 $operator:literal, $visibility:literal, $permission:ident, $ui:literal, $status:literal,
 $destructive:literal, $description:literal)
```

- `$operator`: operator/CLI alias (`"timeline"`, `"changes"`, …) or `""`. Replaces `labels.rs`,
  which is deleted.
- `$destructive`: `true` or `false`. Replaces the name list in `xtask/src/api.rs`.
- `OperationInfo` gains `pub operator_alias: &'static str` and `pub destructive: bool`.
- The `list_workspaces` row gets `$alias = "workspaces"`, `$visibility = "model"`.
  Totals after the cure: 69 operations, 12 model tools, 1 app tool.

Consumers that pattern-match the tuple and must be updated in the same commit by package C:
`crates/contract/src/metadata.rs`, `crates/core/src/ports.rs`, `crates/core/src/dispatch.rs`,
`crates/server/src/lib.rs`, `crates/cli/src/lib.rs` (uses `operation.operator_alias`),
`xtask/src/api.rs`, `tests/support/application.rs`.

### Contract types (package C produces)

```rust
// crates/contract/src/error.rs
pub enum ErrorCode { /* existing variants */ NotImplemented }   // wire: "not_implemented", HTTP 501

pub struct ApiError {
    pub code: ErrorCode,
    pub message: String,
    pub field: Option<String>,
    pub request_id: Option<String>,
    pub detail: Option<Box<ErrorDetail>>,          // boxed: cures clippy::result_large_err
}
impl ApiError {
    pub fn new(code: ErrorCode, message: impl Into<String>) -> Self;
    pub fn with_detail(self, detail: ErrorDetail) -> Self;   // boxes internally
    pub fn with_field(self, field: impl Into<String>) -> Self;
}

pub struct DraftConflictItem {
    pub item_id: ItemId,
    pub draft_base: Revision,
    pub current_revision: Revision,
    pub deleted: bool,
    pub changes: Vec<crate::history::FileChange>,
}
// ErrorDetail::DraftConflict { items: Vec<DraftConflictItem> }   (was a single item + untyped diff)

// crates/contract/src/scope.rs
pub enum Target {
    Authenticated,                      // any signed-in principal; no grant lookup
    Deployment(Permission),
    Workspace(WorkspaceId, Permission),
}
impl Target { pub const fn permission(self) -> Permission; }   // Authenticated reports Permission::Read

pub enum ReplayPolicy {
    StoredResponse,
    AlreadyIssued { id_pointer: &'static str },   // JSON pointer into the response, e.g. "/connector/connector_id"
}
// get_session (Empty), list_workspaces, get_catalog, get_health, get_readiness -> vec![Target::Authenticated]
// CreateConnectorRequest::REPLAY = ReplayPolicy::AlreadyIssued { id_pointer: "/connector/connector_id" }

// crates/contract/src/import.rs
pub enum JobKind { Import, Redigest, ExportWorkspace, BackupWorkspace, RestoreWorkspace, RebuildIndex, ExportView }
// Job gains: pub kind: JobKind

// crates/contract/src/history.rs
// CommitRequest loses `expected_head`. Fields: workspace_id, item_ids (min 1), message, idempotency_key.
// FileChange.old_path / new_path and GetSourcesResponse.appearance get
//   #[serde(default, skip_serializing_if = "Option::is_none")]

// crates/contract/src/views.rs
impl ViewDocument {
    /// Bindings whose source lives outside `workspace`; a saved or presented View must have none.
    pub fn bindings_outside(&self, workspace: WorkspaceId) -> Vec<&ViewBinding>;
}
```

### Dispatch (package E produces; package H consumes)

```rust
// crates/core/src/context.rs
pub enum Attempt { First, Resumed }
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
#[derive(Debug, Clone, Copy)]
pub struct Caller<'a> {
    pub principal: &'a Principal,
    pub session_id: Option<&'a str>,     // browser session; None for bearer/connector callers
}
pub struct DispatchPorts<'a> {
    pub access: &'a dyn AccessControl,
    pub mutations: &'a dyn MutationStore,
}
pub async fn dispatch(
    service: &dyn Application,
    ports: &DispatchPorts<'_>,
    caller: &Caller<'_>,
    operation_id: &str,
    input: serde_json::Value,
) -> Result<serde_json::Value, ApiError>;

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
    /// Ends the lease only if `lease` is the current grant; a stale lease is a no-op. After a
    /// release that took effect, the next `begin` with the same key and digest returns
    /// `Abandoned { lease }`, so the retry runs as `Attempt::Resumed` under the same identity
    /// (the failed attempt may have left effects).
    fn release(&self, lease: MutationLease) -> PortFuture<'_, ()>;
    // Lifetimes are elided wherever only `&self` is borrowed: clippy::elidable_lifetime_names
    // rejects the named form.
}
// `record_effect`, `find` and the `AbandonedEffects` trait are deleted. On New(lease) and
// Abandoned{lease}: context.mutation = Some(lease.mutation_id); on Abandoned also
// context.attempt = Attempt::Resumed; the handler runs. `OperationContext.mutation` stays
// `Option<MutationId>`: handlers get the identity, dispatch keeps the lease and closes the
// row with it.

/// SHA-256 over the typed request re-serialized with object keys sorted; independent of
/// serde_json's `preserve_order` feature.
pub fn request_digest<T: serde::Serialize>(request: &T) -> Result<Digest, ApiError>;
```

Dispatch order is unchanged: validate, decode, `targets()`, authorize every target, build the
context, `MutationStore::begin`, the handler, then `complete` on success or `release` on error.

### Test idiom (package E produces; every later Rust test uses it)

`tests/support/check.rs`, included with `#[path]` like `tests/support/application.rs`. Every
test crate that includes it declares `mod check;` at its crate root and uses every item (the
file carries its own self-tests so no includer trips `dead_code`):

```rust
pub type TestResult = Result<(), Box<dyn std::error::Error>>;
/// The error of a result that must have failed.
pub fn err_of<T: std::fmt::Debug, E>(result: Result<T, E>) -> Result<E, String>;
/// The value of an option that must be present; `what` names it in the failure.
pub fn some<T>(value: Option<T>, what: &str) -> Result<T, String>;
```

Rules: tests return `TestResult`, use `?`, `assert!`/`assert_eq!`, `err_of` and `some`. No
`unwrap`, `expect`, `expect_err`, `panic!`, `unreachable!` or `[]` indexing (use `.get(..)` with
`some`). No `#[allow]` / `#[expect]` anywhere.

### Commands (this machine)

- cargo always runs from PowerShell (Git Bash's `link.exe` shadows MSVC's and every build
  script fails to link). `bun` and `git` may run from either shell.
- Each cure worktree lives under `D:\okf\cure\<name>` and builds into its own `target\`.
- Rust gate for a package: `cargo fmt --all --check`, then
  `cargo clippy --locked -p <crates> --all-targets -- -D warnings`, then
  `cargo test --locked -p <crates>`.

### Commit message format (every commit in this plan)

```text
type(scope): what, in the imperative, ending with a full stop.

Why: the defect or need, with the finding or SPEC sentence it answers.
What changed: the behaviour or signature that is different now.
Verified: the exact command(s) and the observed result.
Next: what the following step is, or "Blocked: <what and on whom>".

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_01LoZr4ME3crLYjcTNn5GcAC
```

Types: feat, fix, test, docs, chore, ci, refactor. One concern per commit. Never amend a pushed
commit, never rebase, never squash.
