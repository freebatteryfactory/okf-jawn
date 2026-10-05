## Package F: Store ports

**Branch:** `cure/ports` (created from `integration/foundation-cure` after package C is merged)
**Worktree:** `D:\okf\cure\ports`
**Wave:** 2, in parallel with D (xtask, ui/scripts), E (dispatch) and G (crates/mcp).

**Files allowed (exact):**

- Modify `crates/core/src/storage.rs`, `jobs.rs`, `credentials.rs`, `sandbox.rs`, `confirmations.rs`, `drafts.rs`, `proposals.rs`, `uploads.rs`, `events.rs`, `conversion.rs`, `search.rs`, `readiness.rs` (no change needed; see Deviations 13)
- Modify `crates/core/src/application/mod.rs`
- Modify `crates/core/src/lib.rs` (module declarations and their one-line docs only)
- Create `crates/core/tests/ports.rs`
- Create `tests/foundation/ports.test.mjs`
- Modify `crates/storage/AGENTS.md`, `crates/ingest/AGENTS.md`

**Must not touch:** `crates/contract/**` (a needed contract change goes in Deviations);
`crates/core/src/{dispatch,mutations,context,access,ports}.rs`;
`crates/core/tests/{dispatch.rs,authorization.rs,support/**}`; `tests/support/**`; every
`Cargo.toml`, `Cargo.lock`, `clippy.toml`; `verification.json`; `api/`, `generated/`.

**SPEC / AGENTS sentences served:**

- SPEC §3: "Revision is a resolved Git commit. At::Latest is a request selector resolved once before the operation reads."
- SPEC §5: "RecordStore is the durable application source of job truth. Acceptance is persisted before enqueue/reporting, and handlers are idempotent against the durable job identity." and "Blob writes, SQLite transactions and Git commits need explicit recovery because they are not one transaction."
- SPEC §8: "every store that creates a durable row (jobs, connectors, comments, uploads, confirmations, drafts) takes the MutationId and enforces it as unique, so a retry after an abandoned lease never duplicates an effect or issues a second connector secret."
- SPEC §7: "The Application owns authorization, workspace scoping, revision resolution and coordinating index updates and rebuilds with content changes."
- SPEC §11: "A newly issued connector secret is returned exactly once and never stored in plain text" and "Principal identity is not authorization."
- SPEC §2: "Core never depends on a concrete storage, ingestion or transport crate."
- `crates/core/AGENTS.md`: "OKF conformance and lint are application policy owned here. … Do not duplicate that policy in storage or ingest."
- Design §3: "A resumed attempt re-runs the handler under the same `MutationId` against stores that treat a repeated id as a no-op returning the prior row. `VersionStore::commit` is idempotent on `MutationId`."

**The one rule every port obeys after this package.** A port method takes a `StorageScope` or
`TenantId`, resolved `Revision`s, ids, and core-owned parameter types. It never takes `At`,
`Principal` or a wire `*Request` type. A method that creates a durable row takes `MutationId`
and, when called again with the same id, writes nothing and returns what the first call
returned.

**Gate and what this package must clear.**

```powershell
cargo fmt --all --check
cargo clippy --locked -p okf-jawn-contract -p okf-jawn-core --all-targets -- -D warnings
cargo test --locked -p okf-jawn-core --test ports
bun test ./tests/foundation/ports.test.mjs
```

Package E cures, in parallel, the lint errors in `crates/core/src/{dispatch,mutations,context,access}.rs`
and in `crates/core/tests/{dispatch.rs,authorization.rs,support/**}`. Until E is merged the Clippy
command above cannot exit 0 in this worktree, and because the library target stops on E-owned
lint errors Clippy never reaches `crates/core/tests/ports.rs`. Package F must clear every
diagnostic whose location is one of its own files:

```text
crates/core/src/{storage,jobs,credentials,sandbox,confirmations,drafts,proposals,uploads,events,conversion,search,readiness}.rs
crates/core/src/application/mod.rs
crates/core/src/lib.rs
crates/core/tests/ports.rs      (checked by the orchestrator once E is merged; written to the lint policy here)
```

At the base commit CI run 37352882339 reports two such diagnostics: `storage.rs:57/77`
(`arbitrary_source_item_ordering`: `impl Provenance` between type definitions) and
`credentials.rs:41` (`needless_lifetimes` on `create_connector`). The filtered check used in the
tasks is:

```powershell
cargo clippy --locked -p okf-jawn-contract -p okf-jawn-core --all-targets --message-format=short -- -D warnings 2>&1 |
  Select-String -Pattern 'crates[\\/]core[\\/]src[\\/](storage|jobs|credentials|sandbox|confirmations|drafts|proposals|uploads|events|conversion|search|readiness|lib|application[\\/]mod)\.rs|crates[\\/]core[\\/]tests[\\/]ports\.rs'
```

Expected: no line printed. Package E's test fixture
`crates/core/tests/dispatch.rs:654-730` implements `ConfirmationStore` including
`find_by_mutation`, which Task F.10 removes from the trait; from then on the `dispatch` test
target does not compile on this branch. Package E's plan deletes that fixture
(`FixtureConfirmationStore`) together with its test, so once both packages are merged nothing
is left to repair; if E keeps it, the orchestrator deletes the one method at integration. The
`ports` test target does not include that file, so `cargo test --locked -p okf-jawn-core --test ports`
is unaffected.

**Formatting.** Format only files this package owns:
`rustfmt --edition 2024 <file>...` (the repository `rustfmt.toml` sets `max_width = 100`).
Never run `cargo fmt --all` without `--check` here: it would rewrite other packages' files.

**Rust rules that bite in these files** (all are deny-level; none may be suppressed):

- Item order per `clippy.toml`: `use`, then types (`type`, `enum`, `struct`, `trait`), then
  `const`, then `impl`, then `fn`. A trait is a type: every `impl` block and free function comes
  after the last trait in the file.
- `needless_lifetimes`: when `&self` is the only reference parameter write
  `fn f(&self, …) -> PortFuture<'_, T>`; use `<'a>` only when a second parameter borrows.
- `doc_markdown`: every identifier in a doc comment is in backticks.
- `missing_docs`: every public item, field, variant and variant field has a `///` line.
- `must_use_candidate`: a pure public function carries `#[must_use]`.
- No `unwrap`, `expect`, `panic!`, indexing, slicing or unchecked arithmetic, in tests too.

**Test idiom note.** `tests/support/check.rs` is produced by package E in this same wave and does
not exist on this branch. `crates/core/tests/ports.rs` declares
`type TestResult = Result<(), Box<dyn Error>>;` itself and uses only `?`, `assert!`,
`assert_eq!`, `assert_ne!`, `matches!` and `.ok_or("…")?`. See Deviations 6.

**How a port is proven without an implementation.** No port has an implementation in Stage 1a
and none may be faked. For each trait, `ports.rs` holds one `async fn …_calls(port: &dyn Trait, …)`
that calls every method; it is never awaited, and its proof is that the file compiles. A
`#[test]` marks it used with `type_checked(&…_calls)`. Pure helpers and plain data flow get
ordinary runtime assertions.

---

### Task F.1: The signature guard, written first and left red

**Files:**

- Create: `tests/foundation/ports.test.mjs` (stays **untracked** until Task F.19)
- Test: itself

**Interfaces:**

- Consumes: the text of `crates/core/src/**/*.rs`.
- Produces: six `bun test` cases. Four are green from the start (the scanner's own rules and
  the coverage check). Two are red until the port files are cured:
  `no port signature mentions At, Principal or a wire request type` (green after Task F.12) and
  `no store keeps a lookup by mutation beside its idempotent insert` (green after Task F.11).
  Task F.15 adds two more cases to this file (a list of writing methods that must take the
  write identity); from then on the file has eight cases, all green after Task F.17.

A red test is never committed, so this task ends without a commit. The file stays in the working
tree; each later task reruns it and watches its own file leave the failure list.

- [ ] **Step 1: Write the guard.** Create `tests/foundation/ports.test.mjs` with exactly:

```js
/** Port signatures name resolved core types only; a dependency-free check that needs no Rust toolchain. */
import test from 'node:test';
import assert from 'node:assert/strict';
import { readFile, readdir } from 'node:fs/promises';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';

const root = fileURLToPath(new URL('../../', import.meta.url));
const source = join(root, 'crates/core/src');

/** Every file that declares a port trait, and whether its signatures may name `Principal`. */
const portFiles = new Map([
  ['access.rs', { principal: true }],
  ['confirmations.rs', { principal: false }],
  ['conversion.rs', { principal: false }],
  ['credentials.rs', { principal: false }],
  ['drafts.rs', { principal: false }],
  ['events.rs', { principal: false }],
  ['jobs.rs', { principal: false }],
  ['mutations.rs', { principal: false }],
  ['proposals.rs', { principal: false }],
  ['readiness.rs', { principal: false }],
  ['sandbox.rs', { principal: false }],
  ['search.rs', { principal: false }],
  ['storage.rs', { principal: false }],
  ['uploads.rs', { principal: false }],
]);
/** `ports.rs` declares `Application` by macro; its methods take the wire request by design. */
const exempt = new Set(['ports.rs']);

const rules = [
  { name: 'the `At` selector', pattern: /\bAt\b/ },
  { name: 'a wire `*Request` type', pattern: /\b\w+Request\b/ },
];
const principalRule = { name: '`Principal`', pattern: /\bPrincipal\b/ };

function stripComments(rust) {
  return rust.replace(/\/\*[\s\S]*?\*\//g, '').replace(/\/\/[^\n]*/g, '');
}

/** Every `fn` signature declared inside a `pub trait` block. */
function traitSignatures(rust) {
  const code = stripComments(rust);
  const found = [];
  const header = /pub\s+trait\s+(\w+)[^{;]*\{/g;
  for (let match = header.exec(code); match; match = header.exec(code)) {
    let depth = 1;
    let end = header.lastIndex;
    while (end < code.length && depth > 0) {
      if (code[end] === '{') depth += 1;
      else if (code[end] === '}') depth -= 1;
      end += 1;
    }
    const body = code.slice(header.lastIndex, end - 1);
    const signature = /\bfn\s+(\w+)[^;{]*/g;
    for (let fn = signature.exec(body); fn; fn = signature.exec(body)) {
      found.push({ trait: match[1], name: fn[1], text: fn[0].replace(/\s+/g, ' ').trim() });
    }
    header.lastIndex = end;
  }
  return found;
}

function violations(file, rust, allowPrincipal) {
  const active = allowPrincipal ? rules : [...rules, principalRule];
  const out = [];
  for (const signature of traitSignatures(rust)) {
    for (const rule of active) {
      if (rule.pattern.test(signature.text)) out.push(`${file}: ${signature.trait}::${signature.name} mentions ${rule.name}`);
    }
  }
  return out;
}

async function rustFiles(directory, prefix = '') {
  const out = [];
  for (const entry of await readdir(directory, { withFileTypes: true })) {
    const relative = prefix ? `${prefix}/${entry.name}` : entry.name;
    if (entry.isDirectory()) out.push(...await rustFiles(join(directory, entry.name), relative));
    else if (entry.name.endsWith('.rs')) out.push(relative);
  }
  return out.sort();
}

/** A creating store returns the prior row for a repeated MutationId; a lookup by mutation is a second mechanism. */
function lookupsByMutation(file, rust) {
  return traitSignatures(rust)
    .filter(signature => /^find_(?:\w+_)?by_mutation$|^find_mutation$/.test(signature.name))
    .map(signature => `${file}: ${signature.trait}::${signature.name}`);
}

const sample = `
/// Docs may say At, Principal and SearchRequest freely.
pub trait Sample: Send + Sync {
    /// Resolve At once.
    fn resolve<'a>(&'a self, at: &'a At) -> PortFuture<'a, Revision>;
    fn list<'a>(&'a self, principal: &'a Principal) -> PortFuture<'a, Vec<Workspace>>;
    fn log<'a>(
        &'a self,
        request: LogRequest,
    ) -> PortFuture<'a, LogResponse>;
    fn head<'a>(&'a self, scope: &'a StorageScope, created_at: String) -> PortFuture<'a, Revision>;
    fn find_by_mutation<'a>(&'a self, mutation_id: MutationId) -> PortFuture<'a, Option<Row>>;
    fn find_comment_by_mutation<'a>(&'a self, mutation_id: MutationId) -> PortFuture<'a, Option<Row>>;
    fn find_commit<'a>(&'a self, mutation_id: MutationId) -> PortFuture<'a, Option<Revision>>;
}
pub struct Record { pub principal: Principal, pub at: At }
pub fn free(request: LogRequest) {}
`;

test('a trait signature naming At, Principal or a request type is reported; docs, fields and free functions are not', () => {
  assert.deepEqual(violations('sample.rs', sample, false), [
    'sample.rs: Sample::resolve mentions the `At` selector',
    'sample.rs: Sample::list mentions `Principal`',
    'sample.rs: Sample::log mentions a wire `*Request` type',
  ]);
});

test('Principal is tolerated only in a file that says so', () => {
  assert.deepEqual(violations('access.rs', sample, true), [
    'access.rs: Sample::resolve mentions the `At` selector',
    'access.rs: Sample::log mentions a wire `*Request` type',
  ]);
});

test('a lookup by mutation is reported by name; other finders are not', () => {
  assert.deepEqual(lookupsByMutation('sample.rs', sample), [
    'sample.rs: Sample::find_by_mutation',
    'sample.rs: Sample::find_comment_by_mutation',
  ]);
});

test('every core file that declares a pub trait is covered by this guard', async () => {
  const present = await rustFiles(source);
  const uncovered = [];
  for (const file of present) {
    const declares = /pub\s+trait\s+\w+/.test(stripComments(await readFile(join(source, file), 'utf8')));
    if (declares && !portFiles.has(file) && !exempt.has(file)) uncovered.push(file);
  }
  assert.deepEqual(uncovered, []);
  for (const file of portFiles.keys()) assert.ok(present.includes(file), `${file} is listed but missing`);
});

test('no port signature mentions At, Principal or a wire request type', async () => {
  const found = [];
  for (const [file, { principal }] of portFiles) {
    found.push(...violations(file, await readFile(join(source, file), 'utf8'), principal));
  }
  assert.deepEqual(found, []);
});

test('no store keeps a lookup by mutation beside its idempotent insert', async () => {
  const found = [];
  for (const file of portFiles.keys()) {
    found.push(...lookupsByMutation(file, await readFile(join(source, file), 'utf8')));
  }
  assert.deepEqual(found, []);
});
```

- [ ] **Step 2: Run it and record the red list.**

```powershell
bun test ./tests/foundation/ports.test.mjs
```

Expected: `4 pass`, `2 fail`. Both lists below were produced by running this guard against
the base sources.

`no port signature mentions At, Principal or a wire request type` fails with these 21 lines in
its `actual` array:

```text
conversion.rs: Converter::convert mentions a wire `*Request` type
credentials.rs: CredentialStore::create_connector mentions a wire `*Request` type
events.rs: EventLog::list mentions a wire `*Request` type
jobs.rs: RecordStore::list_jobs mentions a wire `*Request` type
jobs.rs: RecordStore::list_reviews mentions a wire `*Request` type
proposals.rs: ProposalStore::get mentions a wire `*Request` type
proposals.rs: ProposalStore::list mentions a wire `*Request` type
search.rs: SearchIndex::search mentions a wire `*Request` type
search.rs: SearchIndex::links mentions a wire `*Request` type
search.rs: SearchIndex::graph mentions a wire `*Request` type
storage.rs: VersionStore::resolve mentions the `At` selector
storage.rs: VersionStore::log mentions a wire `*Request` type
storage.rs: VersionStore::diff mentions a wire `*Request` type
storage.rs: VersionStore::blame mentions a wire `*Request` type
storage.rs: WorkspaceCatalog::list mentions `Principal`
storage.rs: WorkspaceCatalog::create mentions `Principal`
storage.rs: WorkspaceCatalog::open mentions `Principal`
storage.rs: WorkspaceCatalog::update mentions a wire `*Request` type
storage.rs: WorkspaceCatalog::update mentions `Principal`
storage.rs: WorkspaceCatalog::archive mentions a wire `*Request` type
storage.rs: WorkspaceCatalog::archive mentions `Principal`
```

`no store keeps a lookup by mutation beside its idempotent insert` fails with these 7 lines:

```text
confirmations.rs: ConfirmationStore::find_by_mutation
credentials.rs: CredentialStore::find_connector_by_mutation
drafts.rs: DraftStore::find_by_mutation
events.rs: EventLog::find_by_mutation
proposals.rs: ProposalStore::find_comment_by_mutation
storage.rs: VersionStore::find_mutation
uploads.rs: UploadStore::find_by_mutation
```

`mutations.rs` and `access.rs` belong to package E and are only read here; neither has a
violating signature before or after E's changes.

- [ ] **Step 3: Do not commit.** Confirm `git status --short` shows exactly
`?? tests/foundation/ports.test.mjs`. In Tasks F.2–F.18 never `git add` this file; every
`git add` in this plan lists exact paths.

---

### Task F.2: Shared parameter types in `storage.rs`; item order fixed

**Files:**

- Modify: `crates/core/src/storage.rs:1-23` (header, imports), `:35` (insert `Page` after `StorageScope`), `:46-73` (`Provenance` and its misplaced `impl`), `:259` (insert `impl` blocks before `workspace_with_permissions`), end of file (append `derive_item_id`)
- Create: `crates/core/tests/ports.rs`
- Test: `crates/core/tests/ports.rs`

Line numbers in every `storage.rs` task are those of the package base commit; after the first
edit, find the item by its name.

**Interfaces:**

- Consumes: `okf_jawn_contract::access::{AccessRoute, Principal}`, `okf_jawn_contract::common::PageRequest`, `sha2::Sha256` (already a dependency of `okf-jawn-core`, see `crates/core/Cargo.toml`), `uuid::Builder::from_custom_bytes(bytes: [u8; 16]).into_uuid()` (uuid 1.27.0, `src/builder.rs:602`, sets the version-8 and variant bits only).
- Produces:

```rust
/// A bounded page of a listing; replaces the wire `PageRequest` in every port signature.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Page { pub cursor: Option<String>, pub limit: u16 }
impl From<PageRequest> for Page;

/// Who a write or a job acts for; `route` is now the typed `AccessRoute`, not a string label.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Provenance { pub subject: String, pub route: AccessRoute, pub client_id: Option<String> }
impl Provenance { #[must_use] pub fn from_principal(principal: &Principal) -> Self; }

/// Same inputs, same identity: a resumed attempt names what its first attempt created.
#[must_use]
pub fn derive_item_id(mutation_id: MutationId, ordinal: u32) -> ItemId;
#[must_use]
pub fn derive_proposal_id(mutation_id: MutationId) -> ProposalId;
```

`Provenance.route` is typed because a job handler builds a `Receipt` (`route: AccessRoute`) and a
commit author from `ClaimedJob.initiator` alone (Task F.7). `derive_item_id` exists because the
caller, not the store, allocates the `ItemId` of every created item (sibling source cards must
reference each other inside one commit, `SourceAppearance.parent_item_id`), and a resumed attempt
re-runs its handler: with random ids the second run would name items the first commit does not
contain. `derive_proposal_id` exists for the same reason: `open_proposal` must name the
candidate reference (`refs/okf-jawn/proposals/<id>`, Task F.4) before any row exists that could
hand a prior id back.

- [ ] **Step 1: Write the failing test.** Create `crates/core/tests/ports.rs` with exactly:

```rust
//! Compile-level and pure-function proofs for the store ports.
//!
//! No port has an implementation in this crate. Each `*_calls` function type-checks one call to
//! every method of a port through `&dyn`; it is never awaited, and its proof is that this file
//! compiles. The other tests run the pure helpers and the data flow between port types.

use std::error::Error;

use okf_jawn_contract::access::{AccessRoute, Principal};
use okf_jawn_contract::common::PageRequest;
use okf_jawn_contract::identity::{MutationId, TenantId};
use okf_jawn_core::storage::{Page, Provenance, derive_item_id, derive_proposal_id};
use uuid::Uuid;

type TestResult = Result<(), Box<dyn Error>>;

#[test]
fn provenance_keeps_the_typed_route_and_client() -> TestResult {
    let principal = Principal {
        subject: "user_1".to_owned(),
        tenant_id: TenantId::try_from("local".to_owned())?,
        route: AccessRoute::McpDelegation,
        client_id: Some("client_1".to_owned()),
        delegation: None,
    };
    assert_eq!(
        Provenance::from_principal(&principal),
        Provenance {
            subject: "user_1".to_owned(),
            route: AccessRoute::McpDelegation,
            client_id: Some("client_1".to_owned()),
        }
    );
    Ok(())
}

#[test]
fn page_carries_the_wire_cursor_and_limit() {
    let page = Page::from(PageRequest {
        cursor: Some("next".to_owned()),
        limit: 25,
    });
    assert_eq!(
        page,
        Page {
            cursor: Some("next".to_owned()),
            limit: 25,
        }
    );
}

#[test]
fn derived_identities_are_stable_per_mutation() {
    let first = MutationId(Uuid::from_u128(1));
    let second = MutationId(Uuid::from_u128(2));
    assert_eq!(derive_item_id(first, 0), derive_item_id(first, 0));
    assert_ne!(derive_item_id(first, 0), derive_item_id(first, 1));
    assert_ne!(derive_item_id(first, 0), derive_item_id(second, 0));
    assert_eq!(derive_item_id(first, 0).0.get_version_num(), 8);
    assert_eq!(derive_proposal_id(first), derive_proposal_id(first));
    assert_ne!(derive_proposal_id(first), derive_proposal_id(second));
    assert_ne!(derive_proposal_id(first).0, derive_item_id(first, 0).0);
}
```

- [ ] **Step 2: Run and watch it fail to compile.** From PowerShell in `D:\okf\cure\ports`:

```powershell
cargo test --locked -p okf-jawn-core --test ports
```

Expected: `error[E0432]: unresolved imports` naming `okf_jawn_core::storage::Page`,
`okf_jawn_core::storage::derive_item_id` and `okf_jawn_core::storage::derive_proposal_id`.

- [ ] **Step 3: Replace the header and imports.** In `crates/core/src/storage.rs` replace lines
1-23 (from `//! Storage interfaces` through `use crate::ports::PortFuture;`) with:

```rust
//! Storage interfaces preserve byte identity, occurrence identity, and revision preconditions.
//!
//! Implementations use selected libraries; they do not infer domain approval from a Git merge.
//! Every commit carries an `Okf-Jawn-Mutation:` trailer naming the durable write identity.
//! Ports take resolved `Revision`s and core parameter types: the application resolves selectors
//! and authorizes the caller before it calls a port.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::pin::Pin;

use okf_jawn_contract::{
    access::{AccessRoute, Permission, Principal},
    common::{MutationResult, PageRequest},
    history::{BlameRequest, BlameResponse, DiffRequest, DiffResponse, LogRequest, LogResponse},
    identity::{
        At, Digest, ItemId, MutationId, ProposalId, Revision, TenantId, WorkspaceId, WorkspacePath,
    },
    item::{ItemDocument, ItemSummary},
    proposal::Change,
    workspace::{ArchiveWorkspaceRequest, UpdateWorkspaceRequest, Workspace},
};
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};
use tokio::io::AsyncRead;

use crate::ports::PortFuture;
```

- [ ] **Step 4: Add `Page`.** Directly after the closing brace of `pub struct StorageScope`
insert:

```rust

/// A bounded page of a listing; the cursor is opaque and scoped to the query that issued it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Page {
    /// Continuation cursor returned with the previous page, if any.
    pub cursor: Option<String>,
    /// Maximum results wanted; an implementation may return fewer.
    pub limit: u16,
}
```

- [ ] **Step 5: Replace `Provenance` and remove its misplaced `impl`.** Replace the whole
`pub struct Provenance` item and the `impl Provenance { … }` block that follows it (base lines
46-73) with only:

```rust
/// Who a write or a job acts for, retained in Git metadata and job records; not a review claim.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Provenance {
    /// Validated identity-provider subject.
    pub subject: String,
    /// Verified authentication route.
    pub route: AccessRoute,
    /// OAuth client when delegation is present.
    pub client_id: Option<String>,
}
```

- [ ] **Step 6: Add the `impl` blocks after the last trait.** Directly before the doc comment
`/// Attach the caller's effective permissions onto a workspace summary.` insert:

```rust
impl From<PageRequest> for Page {
    fn from(page: PageRequest) -> Self {
        Self {
            cursor: page.cursor,
            limit: page.limit,
        }
    }
}

impl Provenance {
    /// Record the authenticated principal as the author of a write or the initiator of a job.
    #[must_use]
    pub fn from_principal(principal: &Principal) -> Self {
        Self {
            subject: principal.subject.clone(),
            route: principal.route.clone(),
            client_id: principal.client_id.clone(),
        }
    }
}

```

- [ ] **Step 7: Append the identity functions.** At the end of the file, after
`workspace_with_permissions`, add:

```rust

/// The identity of the `ordinal`-th item created under one mutation.
///
/// The caller allocates the identity of every item it creates. A resumed attempt re-runs its
/// handler under the same `MutationId`, so deriving identities from that id and a running count
/// makes the repeated edits name exactly the items the first attempt committed.
#[must_use]
pub fn derive_item_id(mutation_id: MutationId, ordinal: u32) -> ItemId {
    ItemId(derived_uuid(b"item", mutation_id, ordinal))
}

/// The identity of the proposal opened under one mutation.
///
/// A resumed attempt derives the same identity, so it finds the candidate reference its first
/// attempt wrote instead of writing a second one.
#[must_use]
pub fn derive_proposal_id(mutation_id: MutationId) -> ProposalId {
    ProposalId(derived_uuid(b"proposal", mutation_id, 0))
}

/// A version-8 UUID from SHA-256 of a label, a mutation identity and a count.
fn derived_uuid(label: &[u8], mutation_id: MutationId, ordinal: u32) -> uuid::Uuid {
    let mut hasher = Sha256::new();
    hasher.update(b"okf-jawn derived identity\0");
    hasher.update(label);
    hasher.update(b"\0");
    hasher.update(mutation_id.0.as_bytes());
    hasher.update(ordinal.to_be_bytes());
    let hash: [u8; 32] = hasher.finalize().into();
    let mut bytes = [0_u8; 16];
    for (target, source) in bytes.iter_mut().zip(hash) {
        *target = source;
    }
    uuid::Builder::from_custom_bytes(bytes).into_uuid()
}
```

- [ ] **Step 8: Format and run.**

```powershell
rustfmt --edition 2024 crates/core/src/storage.rs crates/core/tests/ports.rs
cargo test --locked -p okf-jawn-core --test ports
```

Expected: `test result: ok. 3 passed; 0 failed`.

- [ ] **Step 9: Check this file's lint debt is gone.** Run the filtered Clippy check from the
package header. Expected: no line naming `storage.rs` (the `incorrect ordering of items` error
at `storage.rs:57/77` is cured). A line naming `credentials.rs:41` is still expected; Task F.8
clears it.

- [ ] **Step 10: Commit.**

```powershell
git add crates/core/src/storage.rs crates/core/tests/ports.rs
git commit -m @'
refactor(core): add Page, typed Provenance and derived item identities to storage ports.

Why: port signatures took the wire PageRequest; Provenance carried its route as
a string, so a job could not build a Receipt from its initiator; and impl
Provenance sat between type definitions (clippy arbitrary_source_item_ordering,
storage.rs:57/77 in CI run 37352882339). SPEC 8 needs a retry to duplicate no
effect, which requires a resumed handler to name the same new items.
What changed: storage::Page with From<PageRequest>; Provenance.route is
AccessRoute and Provenance is serializable; impl blocks moved after the last
trait; derive_item_id(mutation_id, ordinal) and derive_proposal_id(mutation_id)
give version-8 UUIDs from SHA-256 of a label, the mutation id and a count.
Verified: cargo test --locked -p okf-jawn-core --test ports -> 3 passed; the
filtered clippy check prints no storage.rs line.
Next: F.3 moves the media type off the blob.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_01LoZr4ME3crLYjcTNn5GcAC
'@
```

---

### Task F.3: `BlobStore` — the media type is not an attribute of the bytes

**Files:**

- Modify: `crates/core/src/storage.rs` — items `ObjectInfo`, `ObjectRead`, `LocalSource`, `BlobStore` (base lines 75-131)
- Modify: `crates/core/tests/ports.rs`
- Test: `crates/core/tests/ports.rs`

**Decision.** The media type is recorded **by the caller that refers to the object**, never by
the blob store: on the source card (`SourceAppearance.media_type`, in Git), on a converted asset
(`ConvertedAsset.media_type`), and on a sandbox capability (`SandboxMint.media_type`, Task F.9).
`ObjectInfo` is digest and size only, and `put` takes no media type.

Why: (1) `object_store` 0.14.2's local filesystem refuses attributes, `src/local.rs:406-410`:

```rust
if !opts.attributes.is_empty() {
    return Err(crate::Error::NotImplemented {
        operation: "`put_opts` with `opts.attributes` specified".into(),
        implementer: self.to_string(),
    });
}
```

(2) Storage cannot detect a type: `infer` is an ingest dependency, not a storage one.
(3) SPEC §4: "Deduplicate storage, not meaning" — the same bytes may be observed under two
occurrences with different detected types; a per-digest type column would let the first writer
win.

**Interfaces:**

- Consumes: `storage::{ByteReader, StorageScope}`, `identity::Digest`.
- Produces:

```rust
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ObjectInfo { pub digest: Digest, pub size: u64 }

pub struct ObjectRead { pub object: ObjectInfo, pub offset: u64, pub length: u64, pub body: ByteReader }

#[derive(Debug, Clone)]
pub struct LocalSource { pub path: PathBuf, pub object: ObjectInfo }

pub trait BlobStore: Send + Sync {
    fn put<'a>(&'a self, scope: &'a StorageScope, body: ByteReader, limit: u64,
               expected: Option<Digest>) -> PortFuture<'a, ObjectInfo>;
    fn open<'a>(&'a self, scope: &'a StorageScope, digest: &'a Digest, offset: u64,
                length: u64) -> PortFuture<'a, ObjectRead>;
    fn materialize<'a>(&'a self, scope: &'a StorageScope, digest: &'a Digest)
        -> PortFuture<'a, LocalSource>;
}
```

- How storage implements it with `object_store` 0.14.2 (`features = ["fs"]`):
  `LocalFileSystem::new_with_prefix(prefix)` rooted at the tenant data directory; `put` streams
  through `ObjectStoreExt::put_multipart(&Path) -> Box<dyn MultipartUpload>` (`put_part(PutPayload)`,
  `complete()`) into a staging key while hashing with SHA-256, then
  `ObjectStoreExt::rename_if_not_exists(from, to)` to the key `sha256/<first two hex>/<digest>`
  (an `AlreadyExists` error means the bytes are already retained: delete the staging key and
  return the existing identity); `open` is `ObjectStoreExt::get_range(&Path, Range<u64>) -> Bytes`
  or `get_opts` with `GetOptions { range: Some(GetRange::Bounded(..)), .. }`; `materialize` is
  `LocalFileSystem::path_to_filesystem(&Path) -> Result<PathBuf>` after `head`.
- Serves SPEC §4: "Bytes are identified by SHA-256." and "Hash possession does not authorize a read."

- [ ] **Step 1: Write the failing test.** In `crates/core/tests/ports.rs`:

Replace the `use okf_jawn_contract::identity::…` and `use okf_jawn_core::storage::…` lines with:

```rust
use okf_jawn_contract::error::ApiError;
use okf_jawn_contract::identity::{Digest, MutationId, TenantId};
use okf_jawn_core::storage::{
    BlobStore, ByteReader, LocalSource, ObjectInfo, Page, Provenance, StorageScope, derive_item_id,
    derive_proposal_id,
};
```

Add after `type TestResult`:

```rust
/// Marks a `*_calls` proof as used without awaiting it: a function item has no runtime size.
fn type_checked<F>(proof: &F) -> bool {
    std::mem::size_of_val(proof) == 0
}

fn digest(fill: char) -> Result<Digest, Box<dyn Error>> {
    Ok(Digest::try_from(fill.to_string().repeat(64))?)
}

async fn blob_store_calls(
    blobs: &dyn BlobStore,
    scope: &StorageScope,
    body: ByteReader,
    expected: &Digest,
) -> Result<LocalSource, ApiError> {
    let stored = blobs.put(scope, body, 1024, Some(expected.clone())).await?;
    let read = blobs.open(scope, &stored.digest, 0, stored.size).await?;
    blobs.materialize(scope, &read.object.digest).await
}
```

Append at the end of the file:

```rust
#[test]
fn object_info_is_digest_and_size_only() -> TestResult {
    let object = ObjectInfo {
        digest: digest('a')?,
        size: 3,
    };
    assert_eq!(object.size, 3);
    assert_eq!(object.digest, digest('a')?);
    assert!(type_checked(&blob_store_calls));
    Ok(())
}
```

- [ ] **Step 2: Run and watch it fail to compile.**

```powershell
cargo test --locked -p okf-jawn-core --test ports
```

Expected:

```text
error[E0063]: missing field `media_type` in initializer of `ObjectInfo`
```

- [ ] **Step 3: Implement.** In `crates/core/src/storage.rs` replace the four items
`ObjectInfo`, `ObjectRead`, `LocalSource` and `BlobStore` (doc comments included; base lines
75-131) with:

```rust
/// Retained byte identity.
///
/// The media type is not stored with the bytes. It is detected per occurrence and recorded by
/// whatever refers to the object: a source card, a converted asset, or a sandbox capability.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ObjectInfo {
    /// Digest verified against stored bytes.
    pub digest: Digest,
    /// Exact byte count.
    pub size: u64,
}

/// A bounded stream and the object identity to which it belongs.
pub struct ObjectRead {
    /// Whole-object identity.
    pub object: ObjectInfo,
    /// Byte offset at which this stream starts.
    pub offset: u64,
    /// Maximum bytes exposed by this reader.
    pub length: u64,
    /// Bounded body owned by the reader.
    pub body: ByteReader,
}

/// Where a retained object lies on the local filesystem, for a bounded converter worker.
#[derive(Debug, Clone)]
pub struct LocalSource {
    /// Absolute path of the retained bytes inside the store. The caller only reads it; the
    /// path carries no file extension and is never an unchecked client pathname.
    pub path: PathBuf,
    /// Identity checked during materialization.
    pub object: ObjectInfo,
}

/// Immutable content-addressed bytes, keyed by tenant and SHA-256 digest.
///
/// Objects are shared inside one tenant and never across tenants; the workspace in `scope` is
/// not part of the key. Knowing a digest authorizes nothing: the application authorizes a read
/// through an item reference before it calls `open`. Use `object_store`, not another protocol.
pub trait BlobStore: Send + Sync {
    /// Store and hash a bounded stream.
    ///
    /// Fails with `TooLarge` past `limit`, and with `Conflict` when `expected` is given and
    /// differs from the computed digest; in both cases nothing is retained. Storing bytes that
    /// are already retained writes nothing and returns the existing identity.
    fn put<'a>(
        &'a self,
        scope: &'a StorageScope,
        body: ByteReader,
        limit: u64,
        expected: Option<Digest>,
    ) -> PortFuture<'a, ObjectInfo>;
    /// Open an already authorized object's selected byte interval; `NotFound` when absent.
    fn open<'a>(
        &'a self,
        scope: &'a StorageScope,
        digest: &'a Digest,
        offset: u64,
        length: u64,
    ) -> PortFuture<'a, ObjectRead>;
    /// Locate retained bytes on the local filesystem for a bounded converter worker.
    fn materialize<'a>(
        &'a self,
        scope: &'a StorageScope,
        digest: &'a Digest,
    ) -> PortFuture<'a, LocalSource>;
}
```

- [ ] **Step 4: Format and run.**

```powershell
rustfmt --edition 2024 crates/core/src/storage.rs crates/core/tests/ports.rs
cargo test --locked -p okf-jawn-core --test ports
```

Expected: `test result: ok. 4 passed; 0 failed`.

- [ ] **Step 5: Commit.**

```powershell
git add crates/core/src/storage.rs crates/core/tests/ports.rs
git commit -m @'
fix(core): record the media type with the reference, not on the blob.

Why: ObjectInfo.media_type had no home. object_store 0.14.2's LocalFileSystem
returns NotImplemented for put_opts with attributes (local.rs:406), storage
has no type detector, and SPEC 4 says to deduplicate storage, not meaning: one
digest may be seen under occurrences with different detected types.
What changed: ObjectInfo is { digest, size } and is Eq. BlobStore docs state
the tenant-and-digest key, the TooLarge and Conflict outcomes of put, and that
materialize returns a read-only path without an extension. The media type
stays on SourceAppearance, ConvertedAsset and (F.9) the sandbox capability.
Verified: cargo test --locked -p okf-jawn-core --test ports -> 4 passed.
Next: F.4 cures VersionStore.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_01LoZr4ME3crLYjcTNn5GcAC
'@
```

---

### Task F.4: `VersionStore` — tree edits, idempotent commit, checked candidate tree

**Files:**

- Modify: `crates/core/src/storage.rs` — the import block; delete `VersionTarget` (base lines 37-44); replace `CommitChanges` and `VersionStore` (base lines 133-226); add `impl TreeEdit` among the `impl` blocks
- Modify: `crates/core/tests/ports.rs`
- Test: `crates/core/tests/ports.rs`

**What is wrong at the base commit.**

- `CommitChanges.changes: Vec<proposal::Change>` (`storage.rs:145`) can only create, edit, move
  and archive. `delete_item`, `set_lifecycle`, `create_folder`, `set_type`, `set_rules`,
  `start_import`, `correct_digest` and `restore_items` have no way to reach Git.
- `resolve` takes `At`; `log`, `diff`, `blame` take wire requests (`storage.rs:151,199-215`).
- `commit` returns `MutationResult`, which holds a `receipt_id` a version store cannot mint.
- `restore` drops `RestoreRequest.message` (`storage.rs:217-225`).
- `create_candidate` carries no author and no message (`storage.rs:181-188`).
- `list` returns items only; `ListItemsResponse.folders` cannot be filled.
- Nothing lets `okf_validator::validate_bundle` see a tree before it is a commit.

**Decision on validation before commit: a check callback, not a staged handle.**

`VersionStore::commit` and `create_candidate` take an `Arc<dyn CandidateCheck>`. Storage
materializes the candidate tree in a private directory, applies the edits, calls
`check.check(&root)`, and only when that returns `Ok` writes the tree and the commit.

Why this and not `stage() -> StagedTree` + `commit_staged()`:

- okf-core 0.2.7 can only see a directory. `Bundle`'s fields are private and its constructors
  are `pub fn load(root: impl AsRef<Path>) -> Result<Self, BundleError>` and `load_file`
  (`src/bundle.rs`). The validator reads the disk again through that root
  (`okf-validator-0.2.7/src/validate.rs:1229` `bundle.root().join("index.md")`, `:1232`
  `fs::read_to_string(path)`), so the directory must exist for the whole check.
- Storage needs that directory anyway: the okf-core operations it must use write files —
  `move_concept(bundle: &Bundle, source: &ConceptId, target: &ConceptId, options: &MoveOptions)`
  and `regenerate_indexes(bundle_root: impl AsRef<Path>) -> io::Result<Vec<PathBuf>>`.
- git2 0.21.0 can fill such a directory from a bare repository:
  `CheckoutBuilder::target_dir(&mut self, dst: &Path)` with `Repository::checkout_tree`
  (libgit2 `checkout.c:2384` only requires a non-bare repository when no target directory is
  given), and can turn it back into a tree without a worktree: `Index::new()` ("Creates a new
  in-memory index"), `Repository::blob_path(&Path) -> Oid`, `Index::add(&IndexEntry)`,
  `Index::write_tree_to(&mut self, repo: &Repository) -> Oid`.
- A handle that outlives the call cannot clean up after itself: a port future cannot run async
  cleanup in `Drop`, core has no temp-directory dependency, and every `?` between `stage` and
  `commit_staged` would leak a directory. With the callback the directory's whole life is one
  function inside storage, under the workspace write lock, so the tree that was checked is the
  tree that is committed.
- Policy stays in core (`crates/core/AGENTS.md`): storage calls an opaque check and never links
  `okf-validator`.

Cleanup, specified:

- The staging directory is `<data>/staging/<tenant id>/<workspace id>/<mutation id>/`, outside
  every user-controlled path and outside the Git directory.
- On return, success or failure, storage removes it (a guard type whose `Drop` calls
  `std::fs::remove_dir_all`; the storage crate has no `tempfile` dependency).
- If the caller drops the future, the blocking task still runs to its end and removes it.
- After a crash the directory may remain. At startup, and before staging for a mutation whose
  directory already exists, storage removes it. Nothing in a staging directory is referenced by
  Git. Blobs a crashed attempt wrote into the object database without moving a reference are
  unreachable and harmless.

**Decision on idempotency.** `commit` is idempotent on `mutation_id` over the range
`expected_head..head`: storage walks it (`Revwalk::push_range("<expected_head>..refs/heads/main")`)
and reads each commit's trailers (`git2::message_trailers_strs(message)`, whose iterator yields
`(&str, &str)` pairs). A match is returned with `replayed: true`. `VersionTarget` is deleted.
One lookup stays on the trait, renamed `find_commit`, because one caller needs it:
`commit_items` has no `expected_head` in its request after package C, so a resumed attempt reads
the current head, which is at or after its own first commit; it must learn that the commit
exists before it compares drafts with a head it moved itself. Every other writer passes the
request's own base revision as `expected_head`, which is stable across attempts.

**Interfaces:**

- Consumes: `storage::{Page, Provenance, StorageScope}`, contract `ItemKind`, `Lifecycle`, `TypeDefinition`, `NamingRules`, `SourceAppearance`, `ItemDocument`, `ItemSummary`, `Change`, `Warning`, `TextRange`, `LogResponse`, `DiffResponse`, `BlameResponse`, `ApiError`.
- Produces (complete code in Step 3):

```rust
pub enum TreeEdit {
    CreateItem { item_id: ItemId, path: WorkspacePath, title: Option<String>, type_name: String,
                 kind: ItemKind, body: String, properties: BTreeMap<String, serde_json::Value> },
    EditItem { item_id: ItemId, body: String, properties: BTreeMap<String, serde_json::Value> },
    MoveItem { item_id: ItemId, destination: WorkspacePath },
    SetLifecycle { item_id: ItemId, lifecycle: Lifecycle },
    DeleteItem { item_id: ItemId },
    CreateFolder { folder: WorkspacePath },
    SetType { definition: TypeDefinition },
    SetRules { rules: NamingRules },
    WriteSourceCard(Box<SourceCard>),
    CorrectDigest { item_id: ItemId, digest: Digest, corrected_markdown: String },
    RestorePaths { from: Revision, paths: Vec<WorkspacePath> },
    RestoreWorkspace { from: Revision },
}
impl TreeEdit {
    #[must_use]
    pub fn from_change(change: Change, new_item_id: ItemId, new_item_kind: ItemKind) -> Self;
}
pub struct SourceCard { pub item_id: ItemId, pub path: WorkspacePath, pub title: String,
    pub type_name: String, pub body: String, pub properties: BTreeMap<String, serde_json::Value>,
    pub appearance: SourceAppearance, pub extraction: Extraction }
pub enum Extraction { Pending, Converted { digest: Digest, partial: bool }, Unsupported,
    Failed { message: String } }

pub struct CommitChanges { pub mutation_id: MutationId, pub expected_head: Revision,
    pub author: Provenance, pub message: String, pub edits: Vec<TreeEdit> }
pub struct CandidateChanges { pub mutation_id: MutationId, pub base: Revision,
    pub author: Provenance, pub message: String, pub edits: Vec<TreeEdit> }
pub struct Promotion { pub mutation_id: MutationId, pub proposal_id: ProposalId,
    pub expected_head: Revision, pub candidate: Revision, pub approver: Provenance,
    pub message: String }
pub struct Committed { pub revision: Revision, pub replayed: bool, pub warnings: Vec<Warning> }
pub struct FolderListing { pub items: Vec<ItemSummary>, pub folders: Vec<WorkspacePath>,
    pub next_cursor: Option<String> }
pub struct LogQuery { pub tip: Revision, pub item_id: Option<ItemId>, pub page: Page }
pub struct DiffQuery { pub from: Revision, pub to: Revision, pub item_id: Option<ItemId> }
pub struct BlameQuery { pub revision: Revision, pub item_id: ItemId, pub lines: TextRange }

pub trait CandidateCheck: Send + Sync {
    fn check(&self, root: &Path) -> Result<Vec<Warning>, ApiError>;
}

pub trait VersionStore: Send + Sync {
    fn head<'a>(&'a self, scope: &'a StorageScope) -> PortFuture<'a, Revision>;
    fn list<'a>(&'a self, scope: &'a StorageScope, revision: &'a Revision,
                folder: Option<&'a WorkspacePath>, page: Page) -> PortFuture<'a, FolderListing>;
    fn show<'a>(&'a self, scope: &'a StorageScope, revision: &'a Revision, item: ItemId)
        -> PortFuture<'a, ItemDocument>;
    fn read_file<'a>(&'a self, scope: &'a StorageScope, revision: &'a Revision,
                     path: &'a WorkspacePath) -> PortFuture<'a, Vec<u8>>;
    fn rules<'a>(&'a self, scope: &'a StorageScope, revision: &'a Revision)
        -> PortFuture<'a, Option<NamingRules>>;
    fn types<'a>(&'a self, scope: &'a StorageScope, revision: &'a Revision)
        -> PortFuture<'a, Vec<TypeDefinition>>;
    fn correction<'a>(&'a self, scope: &'a StorageScope, revision: &'a Revision, item: ItemId,
                      digest: &'a Digest) -> PortFuture<'a, Option<String>>;
    fn commit<'a>(&'a self, scope: &'a StorageScope, changes: CommitChanges,
                  check: Arc<dyn CandidateCheck>) -> PortFuture<'a, Committed>;
    fn find_commit<'a>(&'a self, scope: &'a StorageScope, mutation_id: MutationId,
                       since: &'a Revision) -> PortFuture<'a, Option<Revision>>;
    fn create_candidate<'a>(&'a self, scope: &'a StorageScope, proposal_id: ProposalId,
                            changes: CandidateChanges, check: Arc<dyn CandidateCheck>)
        -> PortFuture<'a, Revision>;
    fn promote_candidate<'a>(&'a self, scope: &'a StorageScope, promotion: Promotion)
        -> PortFuture<'a, Committed>;
    fn log<'a>(&'a self, scope: &'a StorageScope, query: LogQuery) -> PortFuture<'a, LogResponse>;
    fn diff<'a>(&'a self, scope: &'a StorageScope, query: DiffQuery) -> PortFuture<'a, DiffResponse>;
    fn blame<'a>(&'a self, scope: &'a StorageScope, query: BlameQuery)
        -> PortFuture<'a, BlameResponse>;
}
```

Operation → edits, so the storage lane can check coverage of the operation table:

| Operation | Edits |
| --- | --- |
| `create_item`, proposal `Create` | `CreateItem` (a View is `kind: ItemKind::View`; its body is the View note) |
| `commit_items`, proposal `Edit` | `EditItem` per snapshotted draft |
| `move_item`, `apply_names`, proposal `Move` | `MoveItem` per entry, in one commit |
| `set_lifecycle`, proposal `Archive` | `SetLifecycle` |
| `delete_item` | `DeleteItem` |
| `create_folder` | `CreateFolder` |
| `set_type` | `SetType` |
| `set_rules` | `SetRules` |
| `start_import`, `redigest_item` (job) | `WriteSourceCard` per occurrence, plus `MoveItem` when naming rules apply |
| `correct_digest` | `CorrectDigest` |
| `restore_items` | `RestorePaths` (one item: its path at `target`, from `show(target, item).summary.path`) or `RestoreWorkspace` |
| `accept_proposal` | no edits: `promote_candidate` |

How storage implements each method (library calls read from installed source; `refs/heads/main`
stands for whichever head reference storage chooses):

| Method | git2 0.21.0 / okf-core 0.2.7 |
| --- | --- |
| `head` | `Repository::refname_to_id("refs/heads/main") -> Oid` |
| `list`, `show`, `read_file`, `rules`, `types`, `correction` | `Repository::find_commit(oid)`, `Commit::tree()`, `Tree::get_path(&Path) -> TreeEntry`, `Repository::find_blob(oid)`; item files parsed with `okf_core::Document::parse(text)` |
| `commit` | scan as above; stage with `checkout_tree` + `CheckoutBuilder::target_dir`; `MoveItem` through `okf_core::move_concept`; indexes through `okf_core::regenerate_indexes`; item files written with `Document::serialize()`; `check.check(&root)`; tree through `Index::write_tree_to`; `Repository::commit(None, &author, &committer, message, &tree, &[&parent]) -> Oid` with `Signature::now(name, email)`; then `Repository::reference_matching("refs/heads/main", new, true, expected_head_oid, log_message)` — "It will return GIT_EMODIFIED if the reference's value at the time of updating does not match the one passed through `current_id`" |
| `find_commit` | `Revwalk::push_range("<since>..refs/heads/main")` + `git2::message_trailers_strs` |
| `create_candidate` | same staging; `Repository::commit(None, …, &[&base])`; `Repository::reference("refs/okf-jawn/proposals/<id>", oid, false, log_message)` ("return an error if a reference already exists … unless force is true"); on that error read the existing reference and compare its trailer |
| `promote_candidate` | new commit whose tree is the candidate's tree and whose parent is `expected_head`: `Repository::commit(None, &proposer, &approver, message, &candidate.tree()?, &[&head])`, then `reference_matching` |
| `log` | `Repository::revwalk()`, `Revwalk::push(oid)`, `Revwalk::set_sorting` |
| `diff` | `Repository::diff_tree_to_tree(Some(&old), Some(&new), None)`, `Diff::find_similar(None)` for moves, `Patch::from_diff(&diff, idx)` + `Patch::to_buf()` |
| `blame` | `Repository::blame_file(path, Some(&mut opts))` with `BlameOptions::newest_commit(oid)`, `min_line`, `max_line`; `Blame::get_line(lineno) -> Option<BlameHunk>`; `BlameHunk::final_commit_id()` |

Serves SPEC §8 "Create, move, delete, import, Rewind and accepting a proposal remain immediate
commits", "Rewind restores selected historical state in a new snapshot and does not erase
history", "Proposals carry base and proposed revisions", §6 "Applying an accepted preview …
becomes one recoverable versioned change, including link updates", §5 "Generated extraction and
human corrections remain distinguishable. Re-extraction must not erase corrections", §3 "Maintain
folder index.md documents and a change log".

- [ ] **Step 1: Write the failing test.** In `crates/core/tests/ports.rs`:

Replace every `use` line except `use std::error::Error;` and `use uuid::Uuid;` with:

```rust
use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;

use okf_jawn_contract::access::{AccessRoute, Principal};
use okf_jawn_contract::common::{PageRequest, TextRange, Warning};
use okf_jawn_contract::error::{ApiError, ErrorCode};
use okf_jawn_contract::identity::{
    Digest, ItemId, MutationId, ProposalId, Revision, TenantId, WorkspaceId, WorkspacePath,
};
use okf_jawn_contract::item::{ItemKind, Lifecycle};
use okf_jawn_contract::proposal::Change;
use okf_jawn_core::storage::{
    BlameQuery, BlobStore, ByteReader, CandidateChanges, CandidateCheck, CommitChanges, Committed,
    DiffQuery, LocalSource, LogQuery, ObjectInfo, Page, Promotion, Provenance, StorageScope,
    TreeEdit, VersionStore, derive_item_id, derive_proposal_id,
};
```

(`use std::error::Error;` stays in the `std` group with the three new `std` lines; `rustfmt`
orders them.)

Add directly after `type TestResult = …;` and before the first `fn` (item order: types, then
`impl`, then functions):

```rust
/// The application-side check: load the candidate with okf-core and judge it with okf-validator.
struct OkfConformance;

impl CandidateCheck for OkfConformance {
    fn check(&self, root: &Path) -> Result<Vec<Warning>, ApiError> {
        let bundle = okf_core::Bundle::load(root)
            .map_err(|error| ApiError::new(ErrorCode::InvalidInput, error.to_string()))?;
        let report = okf_validator::validate_bundle(&bundle);
        if !report.is_conformant() {
            return Err(ApiError::new(
                ErrorCode::InvalidInput,
                "The candidate tree is not a conformant OKF bundle",
            ));
        }
        Ok(report
            .diagnostics
            .iter()
            .map(|diagnostic| Warning {
                code: diagnostic.severity.as_str().to_owned(),
                message: diagnostic.message.clone(),
                location: diagnostic
                    .path
                    .as_ref()
                    .map(|path| path.display().to_string()),
            })
            .collect())
    }
}
```

Add after the `digest` helper:

```rust
fn revision(fill: char) -> Result<Revision, Box<dyn Error>> {
    Ok(Revision::try_from(fill.to_string().repeat(40))?)
}

fn scope() -> Result<StorageScope, Box<dyn Error>> {
    Ok(StorageScope {
        tenant_id: TenantId::try_from("local".to_owned())?,
        workspace_id: WorkspaceId(Uuid::from_u128(1)),
    })
}

fn initiator() -> Provenance {
    Provenance {
        subject: "user_1".to_owned(),
        route: AccessRoute::LocalOwner,
        client_id: None,
    }
}

async fn version_reads(
    versions: &dyn VersionStore,
    scope: &StorageScope,
    item: ItemId,
    digest: &Digest,
) -> Result<Option<String>, ApiError> {
    let head = versions.head(scope).await?;
    let page = Page {
        cursor: None,
        limit: 50,
    };
    let listing = versions.list(scope, &head, None, page).await?;
    let document = versions.show(scope, &head, item).await?;
    versions
        .read_file(scope, &head, &document.summary.path)
        .await?;
    versions.rules(scope, &head).await?;
    versions.types(scope, &head).await?;
    versions
        .log(
            scope,
            LogQuery {
                tip: head.clone(),
                item_id: Some(item),
                page: Page {
                    cursor: listing.next_cursor,
                    limit: 20,
                },
            },
        )
        .await?;
    versions
        .diff(
            scope,
            DiffQuery {
                from: head.clone(),
                to: head.clone(),
                item_id: None,
            },
        )
        .await?;
    versions
        .blame(
            scope,
            BlameQuery {
                revision: head.clone(),
                item_id: item,
                lines: TextRange { start: 1, end: 5 },
            },
        )
        .await?;
    versions.correction(scope, &head, item, digest).await
}

async fn version_writes(
    versions: &dyn VersionStore,
    scope: &StorageScope,
    changes: CommitChanges,
    proposal_id: ProposalId,
) -> Result<Committed, ApiError> {
    let check: Arc<dyn CandidateCheck> = Arc::new(OkfConformance);
    let mutation_id = changes.mutation_id;
    let base = changes.expected_head.clone();
    let author = changes.author.clone();
    let edits = changes.edits.clone();
    if let Some(found) = versions.find_commit(scope, mutation_id, &base).await? {
        return Ok(Committed {
            revision: found,
            replayed: true,
            warnings: Vec::new(),
        });
    }
    let committed = versions.commit(scope, changes, Arc::clone(&check)).await?;
    let candidate = versions
        .create_candidate(
            scope,
            proposal_id,
            CandidateChanges {
                mutation_id,
                base,
                author: author.clone(),
                message: "Propose a change".to_owned(),
                edits,
            },
            check,
        )
        .await?;
    versions
        .promote_candidate(
            scope,
            Promotion {
                mutation_id,
                proposal_id,
                expected_head: committed.revision,
                candidate,
                approver: author,
                message: "Accept the proposal".to_owned(),
            },
        )
        .await
}
```

Append at the end of the file:

```rust
#[test]
fn version_store_calls_type_check() {
    assert!(type_checked(&version_reads));
    assert!(type_checked(&version_writes));
}

#[test]
fn a_proposed_change_becomes_the_matching_tree_edit() -> TestResult {
    let new_id = ItemId(Uuid::from_u128(10));
    let existing = ItemId(Uuid::from_u128(11));
    let path = WorkspacePath::try_from("notes/plan.md".to_owned())?;
    let created = TreeEdit::from_change(
        Change::Create {
            path: path.clone(),
            type_name: "Note".to_owned(),
            body: "# Plan\n".to_owned(),
            properties: BTreeMap::new(),
        },
        new_id,
        ItemKind::View,
    );
    assert!(matches!(
        created,
        TreeEdit::CreateItem { item_id, kind: ItemKind::View, title: None, path: created_path, .. }
            if item_id == new_id && created_path == path
    ));
    let archived = TreeEdit::from_change(Change::Archive { item_id: existing }, new_id, ItemKind::Note);
    assert!(matches!(
        archived,
        TreeEdit::SetLifecycle { item_id, lifecycle: Lifecycle::Archived } if item_id == existing
    ));
    let moved = TreeEdit::from_change(
        Change::Move {
            item_id: existing,
            destination: path.clone(),
        },
        new_id,
        ItemKind::Note,
    );
    assert!(matches!(
        moved,
        TreeEdit::MoveItem { item_id, destination } if item_id == existing && destination == path
    ));
    let edited = TreeEdit::from_change(
        Change::Edit {
            item_id: existing,
            body: "new".to_owned(),
            properties: BTreeMap::new(),
        },
        new_id,
        ItemKind::Note,
    );
    assert!(matches!(
        edited,
        TreeEdit::EditItem { item_id, body, .. } if item_id == existing && body == "new"
    ));
    Ok(())
}

#[test]
fn a_candidate_check_rejects_a_tree_it_cannot_load() -> TestResult {
    let check: Arc<dyn CandidateCheck> = Arc::new(OkfConformance);
    let error = check
        .check(Path::new("no-such-okf-jawn-candidate-directory"))
        .err()
        .ok_or("a missing candidate directory must be rejected")?;
    assert_eq!(error.code, ErrorCode::InvalidInput);
    Ok(())
}

#[test]
fn a_commit_names_its_mutation_author_base_and_edits() -> TestResult {
    let changes = CommitChanges {
        mutation_id: MutationId(Uuid::from_u128(5)),
        expected_head: revision('a')?,
        author: initiator(),
        message: "Remove a note".to_owned(),
        edits: vec![
            TreeEdit::DeleteItem {
                item_id: derive_item_id(MutationId(Uuid::from_u128(4)), 0),
            },
            TreeEdit::RestoreWorkspace {
                from: revision('b')?,
            },
        ],
    };
    assert_eq!(changes.expected_head, revision('a')?);
    assert_eq!(changes.author, initiator());
    assert_eq!(changes.edits.len(), 2);
    assert_eq!(scope()?.workspace_id, WorkspaceId(Uuid::from_u128(1)));
    Ok(())
}
```

- [ ] **Step 2: Run and watch it fail to compile.**

```powershell
cargo test --locked -p okf-jawn-core --test ports
```

Expected: `error[E0432]: unresolved imports` naming, among others,
`okf_jawn_core::storage::TreeEdit`, `okf_jawn_core::storage::CandidateCheck` and
`okf_jawn_core::storage::Committed`.

- [ ] **Step 3a: Replace the import block** of `crates/core/src/storage.rs` (everything from
`use std::collections::BTreeMap;` through `use crate::ports::PortFuture;`) with:

```rust
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::Arc;

use okf_jawn_contract::{
    access::{AccessRoute, Permission, Principal},
    common::{MutationResult, PageRequest, TextRange, Warning},
    conventions::NamingRules,
    error::ApiError,
    history::{BlameResponse, DiffResponse, LogResponse},
    identity::{
        Digest, ItemId, MutationId, ProposalId, Revision, TenantId, WorkspaceId, WorkspacePath,
    },
    item::{ItemDocument, ItemKind, ItemSummary, Lifecycle, TypeDefinition},
    proposal::Change,
    source::SourceAppearance,
    workspace::{ArchiveWorkspaceRequest, UpdateWorkspaceRequest, Workspace},
};
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};
use tokio::io::AsyncRead;

use crate::ports::PortFuture;
```

- [ ] **Step 3b: Delete `VersionTarget`.** Remove the whole `pub enum VersionTarget` item with
its doc comment and derive line (base lines 37-44).

- [ ] **Step 3c: Replace `CommitChanges` and `VersionStore`.** Replace everything from the doc
comment `/// Complete atomic Git change intent …` through the closing brace of
`pub trait VersionStore` (base lines 133-226) with:

```rust
/// One change to the versioned tree. A commit applies its edits in order, all or nothing.
///
/// The caller allocates the identity of every item it creates; see `derive_item_id`. An edit
/// that names an item or path absent from the tree it is applied to fails the whole commit
/// with `NotFound`; an edit that would overwrite another item's path fails it with `Conflict`.
#[derive(Debug, Clone)]
pub enum TreeEdit {
    /// Create a note or View document at a new path.
    CreateItem {
        /// Identity of the new item.
        item_id: ItemId,
        /// Path of the new document.
        path: WorkspacePath,
        /// Display title; `None` leaves it to the `title` property or, failing that, the file stem.
        title: Option<String>,
        /// User-selected OKF type name.
        type_name: String,
        /// Built-in rendering role.
        kind: ItemKind,
        /// Markdown body.
        body: String,
        /// Complete property map, including unknown extensions.
        properties: BTreeMap<String, serde_json::Value>,
    },
    /// Replace an item's Markdown body and complete property map.
    EditItem {
        /// Item to edit.
        item_id: ItemId,
        /// New Markdown body.
        body: String,
        /// New complete property map, including unknown extensions.
        properties: BTreeMap<String, serde_json::Value>,
    },
    /// Move an item and rewrite the links that point at it.
    MoveItem {
        /// Item to move.
        item_id: ItemId,
        /// New path.
        destination: WorkspacePath,
    },
    /// Set an item's lifecycle; archiving is `Lifecycle::Archived`.
    SetLifecycle {
        /// Item to change.
        item_id: ItemId,
        /// New lifecycle.
        lifecycle: Lifecycle,
    },
    /// Remove an item from the tree; history and retained objects stay.
    DeleteItem {
        /// Item to remove.
        item_id: ItemId,
    },
    /// Create a folder together with its maintained `index.md`.
    CreateFolder {
        /// New folder path.
        folder: WorkspacePath,
    },
    /// Create or replace one user-defined type definition.
    SetType {
        /// Definition to store under its own name.
        definition: TypeDefinition,
    },
    /// Replace the naming rules stored in `.okf/rules.yaml`.
    SetRules {
        /// Complete rule set.
        rules: NamingRules,
    },
    /// Create an import source card, or replace the card that has the same identity.
    ///
    /// Corrections recorded with `CorrectDigest` are kept when a card is replaced.
    WriteSourceCard(Box<SourceCard>),
    /// Record a human correction of one digest, beside the generated extraction.
    CorrectDigest {
        /// Source item whose digest is corrected.
        item_id: ItemId,
        /// Digest the correction applies to.
        digest: Digest,
        /// Corrected Markdown, kept separate from the generated text.
        corrected_markdown: String,
    },
    /// Restore the listed paths to their content at an earlier revision.
    ///
    /// A listed path that is absent at `from` is removed.
    RestorePaths {
        /// Revision to restore from.
        from: Revision,
        /// Paths to restore.
        paths: Vec<WorkspacePath>,
    },
    /// Restore the whole tree to an earlier revision.
    RestoreWorkspace {
        /// Revision to restore from.
        from: Revision,
    },
}

/// One imported source occurrence written as an OKF concept.
#[derive(Debug, Clone)]
pub struct SourceCard {
    /// Identity of the card, allocated by the caller so sibling cards can refer to each other.
    pub item_id: ItemId,
    /// Path of the card in the workspace tree.
    pub path: WorkspacePath,
    /// Display title.
    pub title: String,
    /// User-selected OKF type name.
    pub type_name: String,
    /// Generated extraction shown as the card body; empty unless `extraction` is `Converted`.
    pub body: String,
    /// Preserved extension properties.
    pub properties: BTreeMap<String, serde_json::Value>,
    /// Occurrence metadata: object, observed names, media type, size, parent and successor.
    pub appearance: SourceAppearance,
    /// What the card shows about turning its bytes into text.
    pub extraction: Extraction,
}

/// The extraction state a source card shows; the original bytes are retained in every state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Extraction {
    /// Conversion has not finished.
    Pending,
    /// The card body is a generated digest of the source bytes.
    Converted {
        /// Identity of the retained conversion record in the blob store.
        digest: Digest,
        /// The converter stopped early; the body covers only part of the source.
        partial: bool,
    },
    /// No extractor exists for this format; the body is empty.
    Unsupported,
    /// Conversion failed; the body is empty.
    Failed {
        /// Safe explanation shown with the item.
        message: String,
    },
}

/// One commit on the accepted head.
#[derive(Debug, Clone)]
pub struct CommitChanges {
    /// Durable write identity written as `Okf-Jawn-Mutation:` in the commit.
    pub mutation_id: MutationId,
    /// Head the edits were prepared against; also where the replay search starts.
    pub expected_head: Revision,
    /// Who the commit acts for; not a claim of review.
    pub author: Provenance,
    /// Commit message, without the trailer.
    pub message: String,
    /// Edits applied in order.
    pub edits: Vec<TreeEdit>,
}

/// One retained proposal candidate commit.
#[derive(Debug, Clone)]
pub struct CandidateChanges {
    /// Durable write identity written as `Okf-Jawn-Mutation:` in the candidate commit.
    pub mutation_id: MutationId,
    /// Revision the proposal is drafted against; it need not be the head.
    pub base: Revision,
    /// Who proposes the change.
    pub author: Provenance,
    /// Commit message, without the trailer.
    pub message: String,
    /// Edits applied in order on top of `base`.
    pub edits: Vec<TreeEdit>,
}

/// Acceptance of one exact candidate onto an unchanged head.
#[derive(Debug, Clone)]
pub struct Promotion {
    /// Durable write identity of the acceptance, written as `Okf-Jawn-Mutation:`.
    pub mutation_id: MutationId,
    /// Proposal whose candidate is promoted.
    pub proposal_id: ProposalId,
    /// Head shown when the approver confirmed.
    pub expected_head: Revision,
    /// Exact candidate revision shown when the approver confirmed.
    pub candidate: Revision,
    /// Who accepts; recorded as the committer, while the proposer stays the author.
    pub approver: Provenance,
    /// Commit message, without the trailer.
    pub message: String,
}

/// The commit that carries one mutation.
#[derive(Debug, Clone)]
pub struct Committed {
    /// Commit whose `Okf-Jawn-Mutation:` trailer names the mutation.
    pub revision: Revision,
    /// `true` when that commit already existed and this call wrote nothing.
    pub replayed: bool,
    /// Findings the candidate check returned; empty on a replay.
    pub warnings: Vec<Warning>,
}

/// One page of a folder at one revision.
#[derive(Debug, Clone)]
pub struct FolderListing {
    /// Items directly inside the folder.
    pub items: Vec<ItemSummary>,
    /// Child folders directly inside the folder.
    pub folders: Vec<WorkspacePath>,
    /// Continuation cursor, when more entries remain.
    pub next_cursor: Option<String>,
}

/// History of a workspace or of one item, newest first.
#[derive(Debug, Clone)]
pub struct LogQuery {
    /// Revision to walk back from.
    pub tip: Revision,
    /// Limit history to commits that changed this item, following its moves.
    pub item_id: Option<ItemId>,
    /// Bounded page.
    pub page: Page,
}

/// A comparison of two revisions.
#[derive(Debug, Clone)]
pub struct DiffQuery {
    /// Base revision.
    pub from: Revision,
    /// Compared revision.
    pub to: Revision,
    /// Limit the comparison to this item, following its moves.
    pub item_id: Option<ItemId>,
}

/// Last-change attribution for lines of one item.
#[derive(Debug, Clone)]
pub struct BlameQuery {
    /// Revision whose content is attributed.
    pub revision: Revision,
    /// Item to attribute.
    pub item_id: ItemId,
    /// One-based inclusive lines of the item body, as `show` returns it.
    pub lines: TextRange,
}

/// Application policy run on a candidate tree before it can become a commit.
///
/// The implementation lives in core and decides OKF conformance; storage calls it and never
/// decides. It is synchronous because it reads a directory, and storage calls it from the
/// blocking task that owns that directory.
pub trait CandidateCheck: Send + Sync {
    /// Inspect the complete candidate bundle rooted at `root`.
    ///
    /// # Errors
    /// Returns the error that rejects the write; nothing is committed when this fails.
    fn check(&self, root: &Path) -> Result<Vec<Warning>, ApiError>;
}

/// Versioned workspace content: one Git repository per workspace.
///
/// Every write follows one path inside the implementation, under a per-workspace lock:
/// materialize the base tree in a private staging directory, apply the edits there, maintain
/// the folder indexes and the change log, run the caller's `CandidateCheck` on that directory,
/// write it back as a Git tree, create the commit, and only then move the reference.
///
/// The staging directory is `<data>/staging/<tenant>/<workspace>/<mutation>`. It is removed
/// when the call returns, whether it succeeded or failed, and any directory a crash left there
/// is removed at startup and before it is reused. Nothing in it is ever referenced by Git.
///
/// Reads take a resolved `Revision` and never the current working state.
pub trait VersionStore: Send + Sync {
    /// The accepted head of the workspace.
    fn head<'a>(&'a self, scope: &'a StorageScope) -> PortFuture<'a, Revision>;
    /// List the items and child folders directly inside `folder`; `None` is the root.
    fn list<'a>(
        &'a self,
        scope: &'a StorageScope,
        revision: &'a Revision,
        folder: Option<&'a WorkspacePath>,
        page: Page,
    ) -> PortFuture<'a, FolderListing>;
    /// Read one item's committed content. The returned document never carries a draft.
    fn show<'a>(
        &'a self,
        scope: &'a StorageScope,
        revision: &'a Revision,
        item: ItemId,
    ) -> PortFuture<'a, ItemDocument>;
    /// Read the bytes of any versioned file, such as a folder `index.md`; `NotFound` when absent.
    fn read_file<'a>(
        &'a self,
        scope: &'a StorageScope,
        revision: &'a Revision,
        path: &'a WorkspacePath,
    ) -> PortFuture<'a, Vec<u8>>;
    /// Read the naming rules stored in `.okf/rules.yaml`; `None` when no rules were saved.
    fn rules<'a>(
        &'a self,
        scope: &'a StorageScope,
        revision: &'a Revision,
    ) -> PortFuture<'a, Option<NamingRules>>;
    /// Read every user-defined type definition.
    fn types<'a>(
        &'a self,
        scope: &'a StorageScope,
        revision: &'a Revision,
    ) -> PortFuture<'a, Vec<TypeDefinition>>;
    /// Read the human correction recorded for one digest of a source item, if any.
    fn correction<'a>(
        &'a self,
        scope: &'a StorageScope,
        revision: &'a Revision,
        item: ItemId,
        digest: &'a Digest,
    ) -> PortFuture<'a, Option<String>>;
    /// Apply `changes.edits` in order on top of `changes.expected_head` as one commit.
    ///
    /// Idempotent on `changes.mutation_id`: when a commit carrying this mutation's trailer
    /// already lies after `expected_head` on the head's history, that commit is returned with
    /// `replayed` set, the check is not run and nothing is written. Otherwise, when the head is
    /// not `expected_head`, the call fails with `Conflict` and writes nothing.
    ///
    /// `check` runs once on the staged candidate tree. Its error rejects the commit; its
    /// warnings are returned in the result.
    fn commit<'a>(
        &'a self,
        scope: &'a StorageScope,
        changes: CommitChanges,
        check: Arc<dyn CandidateCheck>,
    ) -> PortFuture<'a, Committed>;
    /// Find the commit after `since` on the head's history that carries `mutation_id`.
    ///
    /// Only a writer whose expected head is not stable across attempts needs this before it
    /// calls `commit`: a Snapshot reads the current head, so on a resumed attempt it must ask
    /// first, with the base revision of one of its drafts as `since`.
    fn find_commit<'a>(
        &'a self,
        scope: &'a StorageScope,
        mutation_id: MutationId,
        since: &'a Revision,
    ) -> PortFuture<'a, Option<Revision>>;
    /// Write a proposal candidate commit on top of `changes.base` and retain it at
    /// `refs/okf-jawn/proposals/<proposal id>`. The head does not move.
    ///
    /// The caller derives `proposal_id` with `derive_proposal_id`, so a resumed attempt names
    /// the same reference.
    ///
    /// Idempotent on `changes.mutation_id`: when the reference already points at a commit
    /// carrying this mutation's trailer, that revision is returned and nothing is written.
    /// `check` runs on the staged candidate tree exactly as it does for `commit`.
    fn create_candidate<'a>(
        &'a self,
        scope: &'a StorageScope,
        proposal_id: ProposalId,
        changes: CandidateChanges,
        check: Arc<dyn CandidateCheck>,
    ) -> PortFuture<'a, Revision>;
    /// Accept a candidate: commit the candidate's exact tree on top of `expected_head`.
    ///
    /// Fails with `Conflict`, writing nothing, when the head is not `expected_head`, when the
    /// candidate's parent is not `expected_head`, or when `candidate` is not the revision the
    /// proposal reference retains. The promoted tree is the tree that was checked when the
    /// candidate was created, so no check runs here. Idempotent on `promotion.mutation_id`
    /// exactly as `commit` is.
    fn promote_candidate<'a>(
        &'a self,
        scope: &'a StorageScope,
        promotion: Promotion,
    ) -> PortFuture<'a, Committed>;
    /// Read retained history.
    fn log<'a>(&'a self, scope: &'a StorageScope, query: LogQuery) -> PortFuture<'a, LogResponse>;
    /// Compare two retained revisions.
    fn diff<'a>(
        &'a self,
        scope: &'a StorageScope,
        query: DiffQuery,
    ) -> PortFuture<'a, DiffResponse>;
    /// Show the commit that last changed each selected line.
    fn blame<'a>(
        &'a self,
        scope: &'a StorageScope,
        query: BlameQuery,
    ) -> PortFuture<'a, BlameResponse>;
}
```

- [ ] **Step 3d: Add `impl TreeEdit`.** Directly before `impl From<PageRequest> for Page` insert:

```rust
impl TreeEdit {
    /// The tree edit a proposed change stands for.
    ///
    /// `new_item_id` and `new_item_kind` are used only for `Change::Create`, which carries
    /// neither an identity nor a rendering role.
    #[must_use]
    pub fn from_change(change: Change, new_item_id: ItemId, new_item_kind: ItemKind) -> Self {
        match change {
            Change::Create {
                path,
                type_name,
                body,
                properties,
            } => Self::CreateItem {
                item_id: new_item_id,
                path,
                title: None,
                type_name,
                kind: new_item_kind,
                body,
                properties,
            },
            Change::Edit {
                item_id,
                body,
                properties,
            } => Self::EditItem {
                item_id,
                body,
                properties,
            },
            Change::Move {
                item_id,
                destination,
            } => Self::MoveItem {
                item_id,
                destination,
            },
            Change::Archive { item_id } => Self::SetLifecycle {
                item_id,
                lifecycle: Lifecycle::Archived,
            },
        }
    }
}

```

- [ ] **Step 4: Format and run.**

```powershell
rustfmt --edition 2024 crates/core/src/storage.rs crates/core/tests/ports.rs
cargo test --locked -p okf-jawn-core --test ports
```

Expected: `test result: ok. 8 passed; 0 failed`.

- [ ] **Step 5: Check the guard and the lints for this file.**

```powershell
bun test ./tests/foundation/ports.test.mjs 2>&1 | Select-String 'VersionStore::'
```

Expected: no line (the four `VersionStore` violations are gone; `WorkspaceCatalog` lines remain
until F.5). Then run the filtered Clippy check from the package header; expected: no
`storage.rs` line.

- [ ] **Step 6: Commit.**

```powershell
git add crates/core/src/storage.rs crates/core/tests/ports.rs
git commit -m @'
feat(core): give VersionStore tree edits, an idempotent commit and a checked candidate tree.

Why: commit took Vec<proposal::Change>, which cannot delete, set lifecycle,
create a folder, save a type or rules, write an import card, record a digest
correction or restore; reads took At and wire requests; restore dropped its
message; candidates had no author; nothing let okf-validator see a tree before
it became a commit (SPEC 3, 5, 6, 8; crates/core/AGENTS.md: conformance is
application policy).
What changed: storage::TreeEdit covers every Git write in the operation table,
with TreeEdit::from_change for proposals. commit and create_candidate take an
Arc<dyn CandidateCheck> that storage runs on the staged directory before it
writes; the staging directory's life and crash cleanup are specified on the
trait. commit is idempotent on MutationId over expected_head..head and returns
Committed { revision, replayed, warnings }. resolve(At) became head();
log/diff/blame take LogQuery/DiffQuery/BlameQuery; list returns FolderListing
with child folders; read_file, rules, types and correction added; restore is
now the RestorePaths/RestoreWorkspace edits in an ordinary commit;
VersionTarget is gone and find_mutation is find_commit, kept for Snapshot.
Verified: cargo test --locked -p okf-jawn-core --test ports -> 8 passed; the
port guard reports no VersionStore line; filtered clippy prints no storage.rs
line.
Next: F.5 keys WorkspaceCatalog by tenant.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_01LoZr4ME3crLYjcTNn5GcAC
'@
```

---

### Task F.5: `WorkspaceCatalog` keyed by tenant, with no `Principal` and no filtering

**Files:**

- Modify: `crates/core/src/storage.rs` — the import block; replace `WorkspaceCatalog` (base lines 228-258)
- Modify: `crates/core/tests/ports.rs`
- Test: `crates/core/tests/ports.rs`

**What is wrong at the base commit.** Every method takes `&Principal` (`storage.rs:231-256`), so
the catalog would decide who sees what; `list` is documented "already filtered for the
authenticated principal". Authorization and listing by grants belong to `AccessControl::grants`
and the application (SPEC §11: "Principal identity is not authorization"). `create` takes a
`properties` map that `CreateWorkspaceRequest { name, description, idempotency_key }` cannot
supply, and no `MutationId`. `update` and `archive` take wire requests and return
`MutationResult`.

**Decisions.** `properties` is dropped (the request type has no such field). `create` makes the
repository and an initial commit so `Workspace.head` is a real revision, which needs an author:
`NewWorkspace.creator`. The catalog returns `Workspace.permissions` empty; the application fills
it with `workspace_with_permissions`. `list` omits archived workspaces and `open` still returns
them, so retained content stays readable (SPEC §3 has no "list archived" surface;
`ListWorkspacesRequest` has no such flag).

**Interfaces:**

- Consumes: `storage::{Provenance, StorageScope}`, `identity::{MutationId, Revision, TenantId}`, `workspace::Workspace`.
- Produces:

```rust
#[derive(Debug, Clone)]
pub struct NewWorkspace { pub name: String, pub description: String, pub creator: Provenance }
#[derive(Debug, Clone)]
pub struct WorkspaceUpdate { pub expected_head: Revision, pub name: String,
    pub description: String, pub author: Provenance }
#[derive(Debug, Clone)]
pub struct WorkspaceArchive { pub expected_head: Revision, pub author: Provenance }

pub trait WorkspaceCatalog: Send + Sync {
    fn list<'a>(&'a self, tenant: &'a TenantId) -> PortFuture<'a, Vec<Workspace>>;
    fn create<'a>(&'a self, tenant: &'a TenantId, mutation_id: MutationId,
                  workspace: NewWorkspace) -> PortFuture<'a, Workspace>;
    fn open<'a>(&'a self, scope: &'a StorageScope) -> PortFuture<'a, Workspace>;
    fn update<'a>(&'a self, scope: &'a StorageScope, mutation_id: MutationId,
                  update: WorkspaceUpdate) -> PortFuture<'a, Workspace>;
    fn archive<'a>(&'a self, scope: &'a StorageScope, mutation_id: MutationId,
                   archive: WorkspaceArchive) -> PortFuture<'a, Workspace>;
}
```

- How storage implements it: a `workspaces` table in SQLite (`rusqlite`) with a unique
  `mutation_id` column per creating row; `create` runs `git2::Repository::init_bare(path)` and
  writes the blank bundle with okf-core's
  `init_bundle(root: impl AsRef<Path>, options: &BundleInitOptions) -> io::Result<Vec<PathBuf>>`
  using `BundleInitOptions { title, create_sample: false, sample_name, author, force }`
  (`create_sample: false` — SPEC §1: "The product ships blank"), then commits it as the first
  revision.
- The application turns the returned `Workspace` into the wire `MutationResult` for
  `update_workspace` and `archive_workspace` (`revision: workspace.head`, plus the receipt it
  inserts).
- Serves SPEC §2 "One hosted tenant may have many workspaces", §11 "Workspace list/open
  responses include the caller's effective permissions".

- [ ] **Step 1: Write the failing test.** In `crates/core/tests/ports.rs`:

Add `use okf_jawn_contract::access::Permission;` by changing the `access` import to
`use okf_jawn_contract::access::{AccessRoute, Permission, Principal};`, add
`use okf_jawn_contract::workspace::Workspace;`, and add `NewWorkspace`, `WorkspaceArchive`,
`WorkspaceCatalog`, `WorkspaceUpdate` and `workspace_with_permissions` to the
`okf_jawn_core::storage::{…}` import.

Add after `version_writes`:

```rust
async fn catalog_calls(
    catalog: &dyn WorkspaceCatalog,
    scope: &StorageScope,
    mutation_id: MutationId,
) -> Result<Vec<Workspace>, ApiError> {
    let created = catalog
        .create(
            &scope.tenant_id,
            mutation_id,
            NewWorkspace {
                name: "Team".to_owned(),
                description: "Shared notes".to_owned(),
                creator: initiator(),
            },
        )
        .await?;
    let opened = catalog.open(scope).await?;
    let updated = catalog
        .update(
            scope,
            mutation_id,
            WorkspaceUpdate {
                expected_head: opened.head,
                name: created.name,
                description: created.description,
                author: initiator(),
            },
        )
        .await?;
    catalog
        .archive(
            scope,
            mutation_id,
            WorkspaceArchive {
                expected_head: updated.head,
                author: initiator(),
            },
        )
        .await?;
    catalog.list(&scope.tenant_id).await
}
```

Append at the end of the file:

```rust
#[test]
fn the_application_not_the_catalog_fills_permissions() -> TestResult {
    let bare = Workspace {
        id: WorkspaceId(Uuid::from_u128(1)),
        name: "Team".to_owned(),
        description: "Shared notes".to_owned(),
        head: revision('a')?,
        created_at: "2026-10-05T00:00:00Z".to_owned(),
        permissions: Vec::new(),
    };
    let shown = workspace_with_permissions(bare, vec![Permission::Read]);
    assert_eq!(shown.permissions, vec![Permission::Read]);
    assert!(type_checked(&catalog_calls));
    Ok(())
}
```

- [ ] **Step 2: Run and watch it fail to compile.**

```powershell
cargo test --locked -p okf-jawn-core --test ports
```

Expected: `error[E0432]: unresolved imports` naming `okf_jawn_core::storage::NewWorkspace`,
`WorkspaceArchive` and `WorkspaceUpdate`.

- [ ] **Step 3a: Replace the import block** of `crates/core/src/storage.rs` with its final form:

```rust
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::Arc;

use okf_jawn_contract::{
    access::{AccessRoute, Permission, Principal},
    common::{PageRequest, TextRange, Warning},
    conventions::NamingRules,
    error::ApiError,
    history::{BlameResponse, DiffResponse, LogResponse},
    identity::{
        Digest, ItemId, MutationId, ProposalId, Revision, TenantId, WorkspaceId, WorkspacePath,
    },
    item::{ItemDocument, ItemKind, ItemSummary, Lifecycle, TypeDefinition},
    proposal::Change,
    source::SourceAppearance,
    workspace::Workspace,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};
use tokio::io::AsyncRead;

use crate::ports::PortFuture;
```

- [ ] **Step 3b: Replace `WorkspaceCatalog`.** Replace everything from the doc comment
`/// Deployment catalog is separate …` through the closing brace of `pub trait WorkspaceCatalog`
(base lines 228-258) with:

```rust
/// A new blank workspace.
#[derive(Debug, Clone)]
pub struct NewWorkspace {
    /// Display name.
    pub name: String,
    /// Purpose of the workspace.
    pub description: String,
    /// Who creates it; recorded as the author of the initial commit.
    pub creator: Provenance,
}

/// New presentation metadata for a workspace.
#[derive(Debug, Clone)]
pub struct WorkspaceUpdate {
    /// Head the caller saw; a moved head conflicts.
    pub expected_head: Revision,
    /// New display name.
    pub name: String,
    /// New purpose.
    pub description: String,
    /// Who makes the change.
    pub author: Provenance,
}

/// Archival of a workspace without deleting anything it retains.
#[derive(Debug, Clone)]
pub struct WorkspaceArchive {
    /// Head the caller saw; a moved head conflicts.
    pub expected_head: Revision,
    /// Who archives it.
    pub author: Provenance,
}

/// The deployment's workspaces, separate from the folder structure inside each one.
///
/// The catalog knows tenants and workspaces, not callers. It never receives a `Principal` and
/// never filters: the application lists by the grants `AccessControl` returns and fills
/// `Workspace::permissions` with `workspace_with_permissions`. The catalog returns that field
/// empty.
pub trait WorkspaceCatalog: Send + Sync {
    /// Every workspace of the tenant that is not archived, unfiltered, in creation order.
    fn list<'a>(&'a self, tenant: &'a TenantId) -> PortFuture<'a, Vec<Workspace>>;
    /// Create a blank workspace: its repository and an initial commit, with no sample content.
    ///
    /// Unique on `mutation_id`: a repeated id creates nothing and returns the prior workspace.
    fn create<'a>(
        &'a self,
        tenant: &'a TenantId,
        mutation_id: MutationId,
        workspace: NewWorkspace,
    ) -> PortFuture<'a, Workspace>;
    /// Read one workspace, archived or not; `NotFound` when the tenant has no such workspace.
    fn open<'a>(&'a self, scope: &'a StorageScope) -> PortFuture<'a, Workspace>;
    /// Replace the name and description.
    ///
    /// Fails with `Conflict` when the head is not `update.expected_head`. A repeated
    /// `mutation_id` changes nothing and returns the workspace as the first call left it.
    fn update<'a>(
        &'a self,
        scope: &'a StorageScope,
        mutation_id: MutationId,
        update: WorkspaceUpdate,
    ) -> PortFuture<'a, Workspace>;
    /// Archive the workspace; its history, objects and records stay.
    ///
    /// Fails with `Conflict` when the head is not `archive.expected_head`. A repeated
    /// `mutation_id` changes nothing and returns the archived workspace.
    fn archive<'a>(
        &'a self,
        scope: &'a StorageScope,
        mutation_id: MutationId,
        archive: WorkspaceArchive,
    ) -> PortFuture<'a, Workspace>;
}
```

- [ ] **Step 4: Format and run.**

```powershell
rustfmt --edition 2024 crates/core/src/storage.rs crates/core/tests/ports.rs
cargo test --locked -p okf-jawn-core --test ports
```

Expected: `test result: ok. 9 passed; 0 failed`.

- [ ] **Step 5: Verify the shape of the finished file.**

```powershell
Select-String -Path crates/core/src/storage.rs -Pattern '^(pub )?(type|struct|enum|trait|impl|fn) ' | ForEach-Object { $_.Line }
```

Expected, in this order (types, then `impl`, then functions):

```text
pub type ByteReader = Pin<Box<dyn AsyncRead + Send + Unpin + 'static>>;
pub struct StorageScope {
pub struct Page {
pub struct Provenance {
pub struct ObjectInfo {
pub struct ObjectRead {
pub struct LocalSource {
pub trait BlobStore: Send + Sync {
pub enum TreeEdit {
pub struct SourceCard {
pub enum Extraction {
pub struct CommitChanges {
pub struct CandidateChanges {
pub struct Promotion {
pub struct Committed {
pub struct FolderListing {
pub struct LogQuery {
pub struct DiffQuery {
pub struct BlameQuery {
pub trait CandidateCheck: Send + Sync {
pub trait VersionStore: Send + Sync {
pub struct NewWorkspace {
pub struct WorkspaceUpdate {
pub struct WorkspaceArchive {
pub trait WorkspaceCatalog: Send + Sync {
impl TreeEdit {
impl From<PageRequest> for Page {
impl Provenance {
pub fn workspace_with_permissions(
pub fn derive_item_id(mutation_id: MutationId, ordinal: u32) -> ItemId {
pub fn derive_proposal_id(mutation_id: MutationId) -> ProposalId {
fn derived_uuid(label: &[u8], mutation_id: MutationId, ordinal: u32) -> uuid::Uuid {
```

Then:

```powershell
bun test ./tests/foundation/ports.test.mjs 2>&1 | Select-String 'storage\.rs:'
```

Expected: no line. Run the filtered Clippy check; expected: no `storage.rs` line.

- [ ] **Step 6: Commit.**

```powershell
git add crates/core/src/storage.rs crates/core/tests/ports.rs
git commit -m @'
fix(core): key WorkspaceCatalog by tenant and take the Principal out of it.

Why: every catalog method took &Principal and list was documented as already
filtered, which put authorization in storage (SPEC 11: principal identity is
not authorization; grants come from AccessControl). create took a properties
map no request can supply and no MutationId; update and archive took wire
requests.
What changed: list(tenant) returns every unarchived workspace unfiltered;
create(tenant, mutation_id, NewWorkspace) makes the repository and an initial
commit and is unique on mutation_id; open(scope); update and archive take
WorkspaceUpdate and WorkspaceArchive plus MutationId and return the Workspace.
The catalog returns permissions empty; the application fills them.
Verified: cargo test --locked -p okf-jawn-core --test ports -> 9 passed; the
port guard reports no storage.rs line; filtered clippy prints no storage.rs
line.
Next: F.6 aligns Converter with Docling.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_01LoZr4ME3crLYjcTNn5GcAC
'@
```

---

### Task F.6: `Converter` — typed settings, a time budget, the structured document and a status

**Files:**

- Modify: `crates/core/src/conversion.rs` (whole file, base lines 1-57)
- Modify: `crates/core/tests/ports.rs`
- Test: `crates/core/tests/ports.rs`

**What is wrong at the base commit.** `ConversionRequest.settings` is an untyped
`BTreeMap<String, serde_json::Value>` (`conversion.rs:20`); there is no timeout; the result keeps
only Markdown, so Docling's structured document is lost; and it has no status, so "partial",
"unsupported" and "failed" cannot be told apart from success (SPEC §5: "Unsupported content is
visible as unsupported, not silently empty or successfully converted"; "No fake success, empty
digest or text-only substitute may stand in for the converter"). The type is also named
`…Request`, which the signature guard forbids.

**What docling 1.93.5 actually takes and returns** (read in `docling-1.93.5/src/`):

```rust
// converter.rs — a consuming builder, then a synchronous call
DocumentConverter::new()
    .document_timeout(timeout: Option<std::time::Duration>)   // "checked between pages"; PDF pipeline only
    .ocr_lang(lang: impl Into<String>)
    .skip_ocr(disable: bool)                                  // "Never run OCR, but keep layout detection and TableFormer"
    .force_full_page_ocr(force: bool)
    .no_table_former(disable: bool)
    .generate_page_images(enabled: bool)
    .convert(&self, source: SourceDocument) -> Result<ConversionResult, ConversionError>
// source.rs — from_file "detect[s] the format from the extension"; a blob path has none
SourceDocument::from_bytes(name: impl Into<String>, format: InputFormat, bytes: Vec<u8>) -> Self
InputFormat::from_extension(ext: &str) -> Option<Self>
// result.rs
pub enum ConversionStatus { Success, PartialSuccess, Failure }
pub struct ErrorItem { pub component_type: String, pub module_name: String, pub error_message: String }
pub struct ConversionResult { pub document: DoclingDocument, pub status: ConversionStatus,
    pub input_name: String, pub format: InputFormat, pub errors: Vec<ErrorItem> }
// error.rs
pub enum ConversionError { Io(..), UnknownFormat { hint }, UnsupportedFormat(InputFormat),
    Parse(String), Streaming(String), Browser(String), Panic(String), Timeout(String), WithSource { .. } }
// docling-core document.rs
DoclingDocument::export_to_markdown(&self) -> String
DoclingDocument::export_to_json(&self) -> String
```

Mapping the ingest lane implements, with no guessing left:

| Core | Docling |
| --- | --- |
| `ConversionInput.file_name` | extension → `InputFormat::from_extension`; `None` → status `Unsupported`, no call |
| `ConversionInput.source.path` | read to bytes → `SourceDocument::from_bytes(file_name, format, bytes)` |
| `ConversionInput.timeout` | `.document_timeout(Some(timeout))`; the worker supervisor still enforces the hard wall-clock and memory bound |
| `OcrPolicy::Auto` / `Skip` / `ForceFullPage` | no call / `.skip_ocr(true)` / `.force_full_page_ocr(true)` |
| `ocr_language: Some(l)` | `.ocr_lang(l)` |
| `table_structure: false` | `.no_table_former(true)` |
| `page_images: true` | `.generate_page_images(true)` |
| `ConversionStatus::Success` / `PartialSuccess` | `Ok` with the same docling status; `ConvertedDocument.markdown` = `export_to_markdown()`, `structured` = a file holding `export_to_json()` |
| `ConversionIssue` | one per `ErrorItem` (`component_type`, `module_name`, `error_message`) |
| `ConversionStatus::Unsupported` | `Err(UnknownFormat)` or `Err(UnsupportedFormat)` |
| `ConversionStatus::Failure` | `Ok` with docling `Failure`, or any other `Err`; the message becomes one `ConversionIssue` |

**Interfaces:**

- Consumes: `storage::LocalSource`, `read::{OutlineEntry, Selection}`, `identity::Digest`.
- Produces: the complete file in Step 3. `ConversionSettings` is `Serialize + Deserialize + Eq`
  with `#[serde(default, deny_unknown_fields)]`, so the application turns
  `RedigestRequest.settings` (a wire map) into it with `serde_json::from_value` and rejects an
  unknown key as `InvalidInput`, and so `JobSpec::Redigest` (Task F.7) can store it.

- [ ] **Step 1: Write the failing test.** In `crates/core/tests/ports.rs` add the imports

```rust
use std::path::PathBuf;
use std::time::Duration;

use okf_jawn_core::conversion::{
    ConversionInput, ConversionSettings, ConversionStatus, Converter, OcrPolicy,
};
use serde_json::json;
```

(merge `PathBuf` into the existing `std::path` import: `use std::path::{Path, PathBuf};`).

Add after `catalog_calls`:

```rust
async fn converter_calls(
    converter: &dyn Converter,
    source: LocalSource,
    output_directory: PathBuf,
) -> Result<ConversionStatus, ApiError> {
    let conversion = converter
        .convert(ConversionInput {
            source,
            file_name: "report.pdf".to_owned(),
            settings: ConversionSettings::default(),
            timeout: Duration::from_secs(120),
            output_directory,
        })
        .await?;
    Ok(conversion.status)
}
```

Append at the end of the file:

```rust
#[test]
fn conversion_settings_are_typed_and_reject_unknown_keys() -> TestResult {
    let defaults = ConversionSettings::default();
    assert_eq!(defaults.ocr, OcrPolicy::Auto);
    assert!(defaults.table_structure);
    assert!(!defaults.page_images);
    let parsed: ConversionSettings =
        serde_json::from_value(json!({"ocr": "force_full_page", "ocr_language": "de"}))?;
    assert_eq!(
        parsed,
        ConversionSettings {
            ocr: OcrPolicy::ForceFullPage,
            ocr_language: Some("de".to_owned()),
            ..ConversionSettings::default()
        }
    );
    assert!(serde_json::from_value::<ConversionSettings>(json!({"quality": "high"})).is_err());
    assert!(type_checked(&converter_calls));
    Ok(())
}
```

- [ ] **Step 2: Run and watch it fail to compile.**

```powershell
cargo test --locked -p okf-jawn-core --test ports
```

Expected: `error[E0432]: unresolved imports` naming `okf_jawn_core::conversion::ConversionInput`,
`ConversionSettings`, `ConversionStatus` and `OcrPolicy`.

- [ ] **Step 3: Implement.** Replace the whole of `crates/core/src/conversion.rs` with:

```rust
//! Conversion returns structural artifacts and locators while retaining original bytes.
//!
//! The port mirrors what the pinned Docling converter takes and returns: a per-document time
//! budget, typed settings, a structured document and a status. A document the converter cannot
//! read is a result whose status is `Unsupported` or `Failure`, never an error: the caller
//! records that state on the source card and keeps the bytes. An error means the worker itself
//! is at fault.

use std::path::PathBuf;
use std::time::Duration;

use okf_jawn_contract::{
    identity::Digest,
    read::{OutlineEntry, Selection},
};
use serde::{Deserialize, Serialize};

use crate::{ports::PortFuture, storage::LocalSource};

/// When optical character recognition runs.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OcrPolicy {
    /// Recognize only regions that have no embedded text layer.
    #[default]
    Auto,
    /// Never run recognition; layout and table structure are still detected.
    Skip,
    /// Recognize every page from its rendered image, ignoring an embedded text layer.
    ForceFullPage,
}

/// Explicit converter settings; they are part of the identity of the digest they produce.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ConversionSettings {
    /// When recognition runs.
    pub ocr: OcrPolicy,
    /// Recognition language, such as `en`; `None` uses the converter's default.
    pub ocr_language: Option<String>,
    /// Reconstruct table structure.
    pub table_structure: bool,
    /// Retain a rendered image of every page.
    pub page_images: bool,
}

/// One document to convert inside a bounded worker.
#[derive(Debug, Clone)]
pub struct ConversionInput {
    /// Verified retained original; its path has no file extension.
    pub source: LocalSource,
    /// Supplied occurrence filename; its extension selects the input format.
    pub file_name: String,
    /// Settings recorded with the result.
    pub settings: ConversionSettings,
    /// Time budget for this document. A converter that runs out returns what it finished with
    /// status `PartialSuccess`; the worker supervisor enforces the hard bound.
    pub timeout: Duration,
    /// New, empty directory controlled by the worker supervisor; every output is written here.
    pub output_directory: PathBuf,
}

/// One derivative generated by a converter, ready for the blob store.
#[derive(Debug, Clone)]
pub struct ConvertedAsset {
    /// File written beneath the output directory.
    pub path: PathBuf,
    /// Detected derivative media type.
    pub media_type: String,
    /// Page, cells, figure, or other original location.
    pub selection: Selection,
    /// Original or generated caption; the origin is not concealed.
    pub caption: Option<String>,
}

/// How a conversion ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConversionStatus {
    /// The whole document was converted.
    Success,
    /// A document was produced but is incomplete; `Conversion::issues` says why.
    PartialSuccess,
    /// No extractor exists for this format; the bytes are retained, nothing was extracted.
    Unsupported,
    /// The converter could not read the document; the bytes are retained.
    Failure,
}

/// One problem the converter recorded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConversionIssue {
    /// Converter component that reported it.
    pub component: String,
    /// Stage that recorded it.
    pub module: String,
    /// What went wrong.
    pub message: String,
}

/// The converter that produced a result; part of the identity of the digest.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConverterIdentity {
    /// Converter name, such as `docling`.
    pub name: String,
    /// Exact converter version.
    pub version: String,
}

/// What a successful or partial conversion extracted.
#[derive(Debug, Clone)]
pub struct ConvertedDocument {
    /// Structured Markdown.
    pub markdown: String,
    /// File beneath the output directory holding the converter's structured document as JSON.
    pub structured: PathBuf,
    /// Navigable source locations.
    pub outline: Vec<OutlineEntry>,
    /// Images, page renders, tables, and other retained assets.
    pub assets: Vec<ConvertedAsset>,
}

/// The outcome of converting one document; never a replacement for the original.
#[derive(Debug, Clone)]
pub struct Conversion {
    /// Identity of the original bytes.
    pub source_digest: Digest,
    /// Converter that ran.
    pub converter: ConverterIdentity,
    /// Settings that were applied.
    pub settings: ConversionSettings,
    /// How the conversion ended.
    pub status: ConversionStatus,
    /// The extraction; present exactly when the status is `Success` or `PartialSuccess`.
    pub document: Option<ConvertedDocument>,
    /// Problems the converter recorded; never empty for a partial, unsupported or failed result.
    pub issues: Vec<ConversionIssue>,
}

/// Bounded worker execution backed by the selected conversion library.
pub trait Converter: Send + Sync {
    /// Convert one document within its time budget.
    ///
    /// Dropping the returned future abandons the result; the worker supervisor owns
    /// cancellation and the memory bound.
    fn convert(&self, input: ConversionInput) -> PortFuture<'_, Conversion>;
}

impl Default for ConversionSettings {
    fn default() -> Self {
        Self {
            ocr: OcrPolicy::Auto,
            ocr_language: None,
            table_structure: true,
            page_images: false,
        }
    }
}
```

- [ ] **Step 4: Format and run.**

```powershell
rustfmt --edition 2024 crates/core/src/conversion.rs crates/core/tests/ports.rs
cargo test --locked -p okf-jawn-core --test ports
```

Expected: `test result: ok. 10 passed; 0 failed`.

- [ ] **Step 5: Guard and lints for this file.**

```powershell
bun test ./tests/foundation/ports.test.mjs 2>&1 | Select-String 'conversion\.rs:'
```

Expected: no line. Filtered Clippy check: no `conversion.rs` line.

- [ ] **Step 6: Commit.**

```powershell
git add crates/core/src/conversion.rs crates/core/tests/ports.rs
git commit -m @'
fix(core): align the Converter port with what Docling takes and returns.

Why: settings were an untyped map, there was no time budget, the structured
document was dropped and there was no status, so a partial, unsupported or
failed conversion looked like success (SPEC 5: unsupported content is visible
as unsupported; no fake success or text-only substitute; heavy conversion runs
with time, memory and cancellation controls).
What changed: ConversionInput { source, file_name, settings, timeout,
output_directory } replaces ConversionRequest; ConversionSettings is typed
(ocr policy, ocr language, table structure, page images), serializable and
rejects unknown keys; Conversion carries the converter identity, the settings,
a status of Success, PartialSuccess, Unsupported or Failure, the issues, and
an optional ConvertedDocument that keeps the structured JSON beside the
Markdown. A document-level failure is a result, not an error.
Verified: cargo test --locked -p okf-jawn-core --test ports -> 10 passed; the
port guard reports no conversion.rs line.
Next: F.7 gives jobs a core-owned specification.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_01LoZr4ME3crLYjcTNn5GcAC
'@
```

---

### Task F.7: Jobs — a core-owned `JobSpec`; a claimed job is enough to do the work

**Files:**

- Modify: `crates/core/src/jobs.rs` (whole file, base lines 1-119)
- Modify: `crates/core/tests/ports.rs`
- Test: `crates/core/tests/ports.rs`

**What is wrong at the base commit.** `create_job(scope, mutation_id, job: Job)` stores only the
wire `Job` (`jobs.rs:55-60`), which holds status and results but none of the request's inputs.
`ClaimedJob { scope, lease, job }` (`jobs.rs:29-36`) therefore tells a handler neither what to
import, nor on whose behalf, nor under which `MutationId` to commit. A worker restarted after a
crash cannot redo the work, so SPEC §5 "handlers are idempotent against the durable job
identity" is unreachable. There is no way to report progress, `JobCompletion` cannot set the
produced items, the artifact or warnings that the wire `Job` shows, `list_jobs` and
`list_reviews` take wire requests, and `insert_review` and `insert_receipt` take no
`MutationId`.

**Decisions.**

- `JobSpec` has one variant per `JobKind` (package C) holding exactly the inputs of the request
  that starts it, with `At` already resolved:

  | Variant | Request | Fields |
  | --- | --- | --- |
  | `Import` | `StartImportRequest` | `base_revision`, `upload_ids`, `destination` (the wire `String`, empty for root, validated into `Option<WorkspacePath>`), `apply_naming_rules` |
  | `Redigest` | `RedigestRequest` | `item_id`, `base_revision`, `settings` (the wire map validated into `ConversionSettings`) |
  | `ExportWorkspace` | `ExportWorkspaceRequest` | `revision` (resolved `at`), `include_history` |
  | `BackupWorkspace` | `BackupWorkspaceRequest` | none |
  | `RestoreWorkspace` | `RestoreWorkspaceRequest` | `artifact_id`, `sha256` |
  | `RebuildIndex` | `RebuildIndexRequest` | none |
  | `ExportView` | `ExportViewRequest` | `item_id`, `revision` (resolved `at`) |

  `workspace_id` is the `StorageScope`; `idempotency_key` became the `MutationId`.
- `JobSpec`, `ConversionSettings` and `Provenance` are `Serialize + Deserialize`, so the record
  store keeps a specification as one JSON column and returns it unchanged on claim.
- The record store allocates the `JobId`; `create_job` returns the `Job` that carries it.
- `update_progress` returns the `Job`, so a handler sees `JobState::Cancelled` and stops (SPEC
  §5: "status, attempts, cancellation, retry and reconciliation survive interruption").
- `list_reviews(scope, item)` takes no revision at all. The store returns every review recorded
  for the item; `Review.coverage` relative to a revision needs the content digest at that
  revision, which only the application can compute through `VersionStore`.
- `insert_receipt` takes `Option<MutationId>`: a read writes a receipt and has no mutation. When
  it is `Some`, the receipt is unique on it, so a resumed write returns the same `receipt_id`.

**Interfaces:**

- Consumes: `storage::{Page, Provenance, StorageScope}`, `conversion::ConversionSettings`, contract `import::{Job, JobKind, ListJobsResponse}`, `review::Review`, `events::Receipt`, `workspace::DownloadArtifact`, `common::Warning`.
- Produces: the complete file in Step 3. Summary:

```rust
pub enum JobSpec { Import { .. }, Redigest { .. }, ExportWorkspace { .. }, BackupWorkspace,
                   RestoreWorkspace { .. }, RebuildIndex, ExportView { .. } }
impl JobSpec { #[must_use] pub const fn kind(&self) -> JobKind; }
pub struct NewJob { pub mutation_id: MutationId, pub initiator: Provenance, pub spec: JobSpec }
pub struct JobLease { pub job_id: JobId, pub token: String, pub attempt: u32 }      // unchanged
pub struct ClaimedJob { pub scope: StorageScope, pub lease: JobLease, pub job: Job,
    pub mutation_id: MutationId, pub initiator: Provenance, pub spec: JobSpec }
pub struct JobCompletion { pub lease: JobLease, pub revision: Option<Revision>,
    pub item_ids: Vec<ItemId>, pub artifact: Option<DownloadArtifact>,
    pub outputs: Vec<Digest>, pub warnings: Vec<Warning> }

pub trait RecordStore: Send + Sync {
    fn create_job<'a>(&'a self, scope: &'a StorageScope, job: NewJob) -> PortFuture<'a, Job>;
    fn get_job<'a>(&'a self, scope: &'a StorageScope, job: JobId) -> PortFuture<'a, Job>;
    fn list_jobs<'a>(&'a self, scope: &'a StorageScope, page: Page) -> PortFuture<'a, ListJobsResponse>;
    fn claim_job(&self, job: JobId) -> PortFuture<'_, Option<ClaimedJob>>;
    fn update_progress<'a>(&'a self, lease: &'a JobLease, progress: u8) -> PortFuture<'a, Job>;
    fn complete_job(&self, completion: JobCompletion) -> PortFuture<'_, Job>;
    fn fail_job(&self, lease: JobLease, message: String, retryable: bool) -> PortFuture<'_, Job>;
    fn cancel_job<'a>(&'a self, scope: &'a StorageScope, job: JobId) -> PortFuture<'a, Job>;
    fn retry_job<'a>(&'a self, scope: &'a StorageScope, job: JobId) -> PortFuture<'a, Job>;
    fn pending_jobs(&self) -> PortFuture<'_, Vec<(StorageScope, JobId)>>;                 // unchanged
    fn expire_leases(&self) -> PortFuture<'_, u32>;                                       // unchanged
    fn insert_review<'a>(&'a self, scope: &'a StorageScope, mutation_id: MutationId,
                         review: Review) -> PortFuture<'a, Review>;
    fn list_reviews<'a>(&'a self, scope: &'a StorageScope, item: ItemId) -> PortFuture<'a, Vec<Review>>;
    fn insert_receipt<'a>(&'a self, scope: &'a StorageScope, mutation_id: Option<MutationId>,
                          receipt: Receipt) -> PortFuture<'a, Receipt>;
    fn get_receipt<'a>(&'a self, scope: &'a StorageScope, receipt: ReceiptId) -> PortFuture<'a, Receipt>;
}
pub trait JobQueue: Send + Sync { fn enqueue(&self, workspace: WorkspaceId, job: JobId) -> PortFuture<'_, ()>; }   // unchanged
pub trait JobHandler: Send + Sync { fn handle<'a>(&'a self, claimed: &'a ClaimedJob) -> PortFuture<'a, ()>; }       // unchanged
```

- How storage implements it: `rusqlite` tables `jobs` (unique `mutation_id`; columns for scope,
  state, attempt, progress, lease token and expiry, the `JobSpec` and `Provenance` as JSON, and
  the completion fields), `reviews` (unique `mutation_id`) and `receipts` (unique `mutation_id`
  where not null). `claim_job` is one `UPDATE … WHERE state = 'queued'` that sets a fresh lease
  token; `complete_job`, `fail_job` and `update_progress` are `UPDATE … WHERE lease_token = ?`.
- Serves SPEC §5 "Acceptance is persisted before enqueue/reporting", §8 "every store that
  creates a durable row … takes the MutationId", §11 "Receipt records identify exactly what this
  server returned".

- [ ] **Step 1: Write the failing test.** In `crates/core/tests/ports.rs` add the imports

```rust
use okf_jawn_contract::events::Receipt;
use okf_jawn_contract::import::{Job, JobKind, JobState};
use okf_jawn_contract::review::Review;
use okf_jawn_core::jobs::{
    ClaimedJob, JobCompletion, JobHandler, JobLease, JobQueue, JobSpec, NewJob, RecordStore,
};
```

and add `ArtifactId`, `JobId` and `UploadId` to the `okf_jawn_contract::identity::{…}` import.

Add after `converter_calls`:

```rust
fn job_specs() -> Result<Vec<JobSpec>, Box<dyn Error>> {
    Ok(vec![
        JobSpec::Import {
            base_revision: revision('a')?,
            upload_ids: vec![UploadId(Uuid::from_u128(9))],
            destination: Some(WorkspacePath::try_from("inbox".to_owned())?),
            apply_naming_rules: true,
        },
        JobSpec::Redigest {
            item_id: ItemId(Uuid::from_u128(10)),
            base_revision: revision('a')?,
            settings: ConversionSettings::default(),
        },
        JobSpec::ExportWorkspace {
            revision: revision('a')?,
            include_history: false,
        },
        JobSpec::BackupWorkspace,
        JobSpec::RestoreWorkspace {
            artifact_id: ArtifactId(Uuid::from_u128(12)),
            sha256: Some(digest('c')?),
        },
        JobSpec::RebuildIndex,
        JobSpec::ExportView {
            item_id: ItemId(Uuid::from_u128(10)),
            revision: revision('a')?,
        },
    ])
}

/// What an import handler does with nothing but the claim it was handed.
fn commit_for(claimed: &ClaimedJob, edits: Vec<TreeEdit>) -> Option<CommitChanges> {
    let JobSpec::Import { base_revision, .. } = &claimed.spec else {
        return None;
    };
    Some(CommitChanges {
        mutation_id: claimed.mutation_id,
        expected_head: base_revision.clone(),
        author: claimed.initiator.clone(),
        message: format!("Import {} source(s)", edits.len()),
        edits,
    })
}

async fn record_store_calls(
    records: &dyn RecordStore,
    scope: &StorageScope,
    new_job: NewJob,
    review: Review,
    receipt: Receipt,
) -> Result<Vec<Review>, ApiError> {
    let mutation_id = new_job.mutation_id;
    let job = records.create_job(scope, new_job).await?;
    records.get_job(scope, job.id).await?;
    let page = Page {
        cursor: None,
        limit: 50,
    };
    records.list_jobs(scope, page).await?;
    if let Some(claimed) = records.claim_job(job.id).await? {
        records.update_progress(&claimed.lease, 40).await?;
        records
            .complete_job(JobCompletion {
                lease: claimed.lease.clone(),
                revision: None,
                item_ids: Vec::new(),
                artifact: None,
                outputs: Vec::new(),
                warnings: Vec::new(),
            })
            .await?;
        records
            .fail_job(claimed.lease, "converter stopped".to_owned(), true)
            .await?;
    }
    records.cancel_job(scope, job.id).await?;
    records.retry_job(scope, job.id).await?;
    records.pending_jobs().await?;
    records.expire_leases().await?;
    let item_id = review.source.item_id;
    records.insert_review(scope, mutation_id, review).await?;
    let stored = records
        .insert_receipt(scope, Some(mutation_id), receipt)
        .await?;
    records.get_receipt(scope, stored.id).await?;
    records.list_reviews(scope, item_id).await
}

async fn job_runtime_calls(
    queue: &dyn JobQueue,
    handler: &dyn JobHandler,
    claimed: &ClaimedJob,
) -> Result<(), ApiError> {
    queue
        .enqueue(claimed.scope.workspace_id, claimed.lease.job_id)
        .await?;
    handler.handle(claimed).await
}
```

Append at the end of the file:

```rust
#[test]
fn every_job_spec_reports_its_kind_and_survives_storage() -> TestResult {
    let specs = job_specs()?;
    let kinds: Vec<JobKind> = specs.iter().map(JobSpec::kind).collect();
    assert!(matches!(
        kinds.as_slice(),
        [
            JobKind::Import,
            JobKind::Redigest,
            JobKind::ExportWorkspace,
            JobKind::BackupWorkspace,
            JobKind::RestoreWorkspace,
            JobKind::RebuildIndex,
            JobKind::ExportView
        ]
    ));
    for spec in &specs {
        let stored = serde_json::to_value(spec)?;
        let loaded: JobSpec = serde_json::from_value(stored)?;
        assert_eq!(&loaded, spec);
    }
    assert_eq!(
        serde_json::to_value(JobSpec::RebuildIndex)?,
        json!({"kind": "rebuild_index"})
    );
    Ok(())
}

#[test]
fn a_commit_is_built_from_a_claimed_job_alone() -> TestResult {
    let scope = scope()?;
    let spec = JobSpec::Import {
        base_revision: revision('a')?,
        upload_ids: vec![UploadId(Uuid::from_u128(9))],
        destination: None,
        apply_naming_rules: true,
    };
    let mutation_id = MutationId(Uuid::from_u128(5));
    let job_id = JobId(Uuid::from_u128(7));
    let claimed = ClaimedJob {
        scope: scope.clone(),
        lease: JobLease {
            job_id,
            token: "claim-1".to_owned(),
            attempt: 1,
        },
        job: Job {
            id: job_id,
            workspace_id: scope.workspace_id,
            kind: spec.kind(),
            state: JobState::Running,
            progress: 0,
            attempt: 1,
            warnings: Vec::new(),
            error: None,
            revision: None,
            item_ids: Vec::new(),
            artifact: None,
        },
        mutation_id,
        initiator: initiator(),
        spec,
    };
    let folder = WorkspacePath::try_from("inbox".to_owned())?;
    let commit = commit_for(&claimed, vec![TreeEdit::CreateFolder { folder }])
        .ok_or("an import job must yield a commit")?;
    assert_eq!(commit.mutation_id, mutation_id);
    assert_eq!(commit.expected_head, revision('a')?);
    assert_eq!(commit.author, initiator());
    assert_eq!(commit.message, "Import 1 source(s)");
    assert_eq!(commit.edits.len(), 1);
    assert_eq!(
        derive_item_id(claimed.mutation_id, 0),
        derive_item_id(commit.mutation_id, 0)
    );
    assert!(type_checked(&record_store_calls));
    assert!(type_checked(&job_runtime_calls));
    Ok(())
}
```

- [ ] **Step 2: Run and watch it fail to compile.**

```powershell
cargo test --locked -p okf-jawn-core --test ports
```

Expected: `error[E0432]: unresolved imports` naming `okf_jawn_core::jobs::JobSpec` and
`okf_jawn_core::jobs::NewJob`. (This is the proof named in the design: at the base commit
`ClaimedJob` has no `mutation_id`, `initiator` or `spec`, so `commit_for` cannot be written.)

- [ ] **Step 3: Implement.** Replace the whole of `crates/core/src/jobs.rs` with:

```rust
//! Durable application work records are independent of queue delivery and telemetry.
//!
//! `RecordStore` is the durable source of job truth. `JobQueue` delivers wake-ups.
//! `JobHandler` executes claimed work; ingest owns the runtime adapter implementation.
//! A job is created from a core-owned `JobSpec`, never from the wire `Job`: the specification
//! carries the inputs, the initiator and the `MutationId` a handler needs to do the work again
//! after a crash. Every insert is unique on `MutationId` and a repeated id returns the prior row.

use okf_jawn_contract::{
    common::Warning,
    events::Receipt,
    identity::{
        ArtifactId, Digest, ItemId, JobId, MutationId, ReceiptId, Revision, UploadId, WorkspaceId,
        WorkspacePath,
    },
    import::{Job, JobKind, ListJobsResponse},
    review::Review,
    workspace::DownloadArtifact,
};
use serde::{Deserialize, Serialize};

use crate::conversion::ConversionSettings;
use crate::ports::PortFuture;
use crate::storage::{Page, Provenance, StorageScope};

/// What a job must do: the inputs of the request that started it, with selectors resolved.
///
/// The record store keeps the specification as written and hands it back on every claim.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum JobSpec {
    /// Convert finalized uploads into source cards.
    Import {
        /// Revision the import is based on.
        base_revision: Revision,
        /// Completed upload slots to import.
        upload_ids: Vec<UploadId>,
        /// Destination folder; `None` is the workspace root.
        destination: Option<WorkspacePath>,
        /// Apply the workspace naming rules to the new cards.
        apply_naming_rules: bool,
    },
    /// Convert one source again with explicit settings, keeping human corrections.
    Redigest {
        /// Source item to convert again.
        item_id: ItemId,
        /// Revision the new digest is based on.
        base_revision: Revision,
        /// Settings that identify the new digest.
        settings: ConversionSettings,
    },
    /// Build a portable export of the workspace.
    ExportWorkspace {
        /// Resolved revision to export.
        revision: Revision,
        /// Include retained history in addition to the selected state.
        include_history: bool,
    },
    /// Back up content, retained objects and application records.
    BackupWorkspace,
    /// Restore content and application records from a retained backup artifact.
    RestoreWorkspace {
        /// Backup artifact to restore from.
        artifact_id: ArtifactId,
        /// Digest the artifact must have before the restore begins, when supplied.
        sha256: Option<Digest>,
    },
    /// Rebuild the derived search and link index at the current head.
    RebuildIndex,
    /// Export one View with its specification, sources and data table.
    ExportView {
        /// View item to export.
        item_id: ItemId,
        /// Resolved revision to export.
        revision: Revision,
    },
}

/// A job to register durably before it is enqueued or reported.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewJob {
    /// Durable write identity of the request that starts the job; every commit and row the
    /// handler creates is written under it.
    pub mutation_id: MutationId,
    /// Who the job acts for.
    pub initiator: Provenance,
    /// What the job must do.
    pub spec: JobSpec,
}

/// Lease identity prevents a late worker from completing a newer attempt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JobLease {
    /// Stable application work id.
    pub job_id: JobId,
    /// Unique claim token changed on every claim.
    pub token: String,
    /// Monotonic attempt number.
    pub attempt: u32,
}

/// A claimed job: everything `JobHandler::handle` needs, with nothing to look up elsewhere.
#[derive(Debug, Clone)]
pub struct ClaimedJob {
    /// Tenant and workspace scope for the work.
    pub scope: StorageScope,
    /// Compare-and-set claim identity.
    pub lease: JobLease,
    /// Durable job record as it stands at the claim.
    pub job: Job,
    /// Durable write identity the job was created under; the same on every attempt.
    pub mutation_id: MutationId,
    /// Who the job acts for.
    pub initiator: Provenance,
    /// What the job must do.
    pub spec: JobSpec,
}

/// A durable completion result, never inferred solely from a queue acknowledgment.
#[derive(Debug, Clone)]
pub struct JobCompletion {
    /// Compare-and-set claim identity.
    pub lease: JobLease,
    /// Committed content revision, when the job changed the workspace.
    pub revision: Option<Revision>,
    /// Items the job produced.
    pub item_ids: Vec<ItemId>,
    /// Export or backup artifact the job produced.
    pub artifact: Option<DownloadArtifact>,
    /// Retained output objects, kept as garbage-collection roots.
    pub outputs: Vec<Digest>,
    /// Non-fatal issues recorded during the work.
    pub warnings: Vec<Warning>,
}

/// SQLite-backed non-rebuildable application records.
pub trait RecordStore: Send + Sync {
    /// Register a queued job and allocate its identity.
    ///
    /// Unique on `job.mutation_id`: a repeated id inserts nothing and returns the prior job,
    /// whatever state it has reached. The returned `Job::kind` is `job.spec.kind()`.
    fn create_job<'a>(&'a self, scope: &'a StorageScope, job: NewJob) -> PortFuture<'a, Job>;
    /// Read one durable job within workspace scope.
    fn get_job<'a>(&'a self, scope: &'a StorageScope, job: JobId) -> PortFuture<'a, Job>;
    /// List durable jobs for a workspace, newest first, with bounded pagination.
    fn list_jobs<'a>(
        &'a self,
        scope: &'a StorageScope,
        page: Page,
    ) -> PortFuture<'a, ListJobsResponse>;
    /// Claim runnable work under a compare-and-set lease; `None` when it is not runnable.
    fn claim_job(&self, job: JobId) -> PortFuture<'_, Option<ClaimedJob>>;
    /// Record progress, 0 to 100, for the current claim and return the job as it now stands.
    ///
    /// A handler reads the returned state: a cancelled job stops working.
    fn update_progress<'a>(&'a self, lease: &'a JobLease, progress: u8) -> PortFuture<'a, Job>;
    /// Commit completion only for the current unexpired claim.
    fn complete_job(&self, completion: JobCompletion) -> PortFuture<'_, Job>;
    /// Keep the failure and retry eligibility for the current claim.
    fn fail_job(&self, lease: JobLease, message: String, retryable: bool) -> PortFuture<'_, Job>;
    /// Cancel pending or running work while retaining recorded state; repeating it changes nothing.
    fn cancel_job<'a>(&'a self, scope: &'a StorageScope, job: JobId) -> PortFuture<'a, Job>;
    /// Queue a failed job again under the same durable identity; repeating it changes nothing.
    fn retry_job<'a>(&'a self, scope: &'a StorageScope, job: JobId) -> PortFuture<'a, Job>;
    /// Enumerate unfinished records across tenants for queue reconciliation on restart.
    fn pending_jobs(&self) -> PortFuture<'_, Vec<(StorageScope, JobId)>>;
    /// Release claims whose leases have expired so work can be reclaimed.
    fn expire_leases(&self) -> PortFuture<'_, u32>;
    /// Record exact reviewed content after application-level confirmation.
    ///
    /// Unique on `mutation_id`: a repeated id inserts nothing and returns the prior review.
    fn insert_review<'a>(
        &'a self,
        scope: &'a StorageScope,
        mutation_id: MutationId,
        review: Review,
    ) -> PortFuture<'a, Review>;
    /// Every review recorded for an item, oldest first, with coverage as it was recorded.
    ///
    /// The application recomputes coverage against the revision it resolved.
    fn list_reviews<'a>(
        &'a self,
        scope: &'a StorageScope,
        item: ItemId,
    ) -> PortFuture<'a, Vec<Review>>;
    /// Persist what was returned, not merely a trace identifier.
    ///
    /// A write passes its `MutationId`: the receipt is then unique on it and a repeated id
    /// returns the prior receipt. A read passes `None` and always inserts.
    fn insert_receipt<'a>(
        &'a self,
        scope: &'a StorageScope,
        mutation_id: Option<MutationId>,
        receipt: Receipt,
    ) -> PortFuture<'a, Receipt>;
    /// Read one durable receipt within workspace scope.
    fn get_receipt<'a>(
        &'a self,
        scope: &'a StorageScope,
        receipt: ReceiptId,
    ) -> PortFuture<'a, Receipt>;
}

/// Delivery adapter; job truth remains in `RecordStore`.
pub trait JobQueue: Send + Sync {
    /// Deliver an existing durable job identity, accepting possible duplicate delivery.
    fn enqueue(&self, workspace: WorkspaceId, job: JobId) -> PortFuture<'_, ()>;
}

/// Executes claimed durable work; ingest owns the Tokio + `RecordStore` runtime adapter.
pub trait JobHandler: Send + Sync {
    /// Run one claimed attempt; completion and failure are written through `RecordStore`.
    fn handle<'a>(&'a self, claimed: &'a ClaimedJob) -> PortFuture<'a, ()>;
}

impl JobSpec {
    /// The wire kind shown on the job.
    #[must_use]
    pub const fn kind(&self) -> JobKind {
        match self {
            Self::Import { .. } => JobKind::Import,
            Self::Redigest { .. } => JobKind::Redigest,
            Self::ExportWorkspace { .. } => JobKind::ExportWorkspace,
            Self::BackupWorkspace => JobKind::BackupWorkspace,
            Self::RestoreWorkspace { .. } => JobKind::RestoreWorkspace,
            Self::RebuildIndex => JobKind::RebuildIndex,
            Self::ExportView { .. } => JobKind::ExportView,
        }
    }
}
```

- [ ] **Step 4: Format and run.**

```powershell
rustfmt --edition 2024 crates/core/src/jobs.rs crates/core/tests/ports.rs
cargo test --locked -p okf-jawn-core --test ports
```

Expected: `test result: ok. 12 passed; 0 failed`.

- [ ] **Step 5: Guard and lints for this file.**

```powershell
bun test ./tests/foundation/ports.test.mjs 2>&1 | Select-String 'jobs\.rs:'
```

Expected: no line. Filtered Clippy check: no `jobs.rs` line.

- [ ] **Step 6: Commit.**

```powershell
git add crates/core/src/jobs.rs crates/core/tests/ports.rs
git commit -m @'
feat(core): create jobs from a core-owned JobSpec so a claim is enough to redo the work.

Why: create_job stored only the wire Job, which has no inputs, initiator or
write identity, so a handler restarted after a crash could not know what to
import or under which MutationId to commit (SPEC 5: handlers are idempotent
against the durable job identity; SPEC 8: every creating store takes the
MutationId). There was no progress update, completion could not set produced
items, artifact or warnings, and list_jobs and list_reviews took wire requests.
What changed: JobSpec has one variant per JobKind with the request inputs and
resolved revisions, JobSpec::kind(), and serde derives; create_job takes
NewJob { mutation_id, initiator, spec }; ClaimedJob carries mutation_id,
initiator and spec; update_progress returns the Job; JobCompletion sets
revision, item_ids, artifact, outputs and warnings; insert_review takes
MutationId, insert_receipt takes Option<MutationId>; list_jobs takes Page;
list_reviews takes the item only.
Verified: cargo test --locked -p okf-jawn-core --test ports -> 12 passed,
including a_commit_is_built_from_a_claimed_job_alone; the port guard reports
no jobs.rs line.
Next: F.8 cures connector issuance.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_01LoZr4ME3crLYjcTNn5GcAC
'@
```

---

### Task F.8: Credentials — `Issued` or `Existing`, a rotate call, one hash function

**Files:**

- Modify: `crates/core/src/credentials.rs` (whole file, base lines 1-63)
- Modify: `crates/core/tests/ports.rs`
- Test: `crates/core/tests/ports.rs`

**What is wrong at the base commit.** `create_connector` returns `IssuedConnector`
(`credentials.rs:41-45`), a type that always holds a secret, while its own doc says a retry
"returns the existing connector without issuing a second secret" — the signature cannot say
that. It takes the wire `CreateConnectorRequest`. `find_connector_by_mutation`
(`credentials.rs:47-50`) exists only for the removed `AbandonedEffects`. `lookup_connector`
takes the plaintext secret and no hash function is named anywhere. `list_connectors` ignores
`ListConnectorsRequest.include_revoked`. Clippy reports `needless_lifetimes` at
`credentials.rs:41`.

**Decisions.**

- `create_connector(mutation_id, NewConnector) -> ConnectorIssue`. `Issued` carries the one-time
  secret; `Existing` means a row with this `MutationId` already exists and its secret was never
  delivered or can no longer be shown.
- `rotate_connector_secret(connector_id) -> IssuedConnector` replaces the stored hash. Design §3:
  "A resumed `create_connector` rotates the secret for the existing connector and returns it."
  A completed one never reaches the store: dispatch replays `already_issued` from the ledger.
- The hash is `credentials::secret_hash`: SHA-256 of the secret's UTF-8 bytes. The store calls it
  when it persists a secret it generated; the authenticating caller calls it before
  `lookup_connector`. The store generates at least 32 bytes from the operating system's random
  source, so a plain SHA-256 (no salt, no stretching) is sufficient and lookup stays a single
  indexed read.
- `find_connector_by_mutation` is removed.

**Interfaces:**

- Consumes: `access::{Connector, IssuedConnector, Principal}`, `identity::{ConnectorId, MutationId, WorkspaceId}`, `sha2::Sha256`.
- Produces:

```rust
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewConnector { pub label: String, pub workspace_ids: Vec<WorkspaceId>, pub allow_propose: bool }
#[derive(Debug, Clone)]
pub enum ConnectorIssue { Issued(IssuedConnector), Existing(Connector) }

pub trait CredentialStore: Send + Sync {
    fn installation_identity(&self) -> PortFuture<'_, InstallationIdentity>;                 // unchanged
    fn create_connector(&self, mutation_id: MutationId, connector: NewConnector)
        -> PortFuture<'_, ConnectorIssue>;
    fn rotate_connector_secret(&self, connector: ConnectorId) -> PortFuture<'_, IssuedConnector>;
    fn list_connectors(&self, include_revoked: bool) -> PortFuture<'_, Vec<Connector>>;
    fn revoke_connector(&self, connector: ConnectorId) -> PortFuture<'_, Connector>;         // unchanged
    fn lookup_connector(&self, hash: [u8; 32]) -> PortFuture<'_, Option<Connector>>;
    fn insert_session(&self, session: SessionRecord) -> PortFuture<'_, SessionRecord>;       // unchanged
    fn get_session<'a>(&'a self, session_id: &'a str) -> PortFuture<'a, Option<SessionRecord>>;  // unchanged
    fn revoke_session<'a>(&'a self, session_id: &'a str) -> PortFuture<'a, ()>;              // unchanged
}
#[must_use]
pub fn secret_hash(secret: &str) -> [u8; 32];
```

- Serves SPEC §11: "A newly issued connector secret is returned exactly once and never stored in
  plain text; idempotent retries of create_connector return AlreadyIssued without the secret."

- [ ] **Step 1: Write the failing test.** In `crates/core/tests/ports.rs` add the imports

```rust
use std::fmt::Write as _;

use okf_jawn_contract::access::{Connector, IssuedConnector};
use okf_jawn_core::credentials::{
    ConnectorIssue, CredentialStore, NewConnector, SessionRecord, secret_hash,
};
```

(merge `Connector` and `IssuedConnector` into the existing `okf_jawn_contract::access::{…}`
import).

Add after `job_runtime_calls`:

```rust
fn hex(bytes: &[u8]) -> Result<String, std::fmt::Error> {
    let mut text = String::new();
    for byte in bytes {
        write!(text, "{byte:02x}")?;
    }
    Ok(text)
}

/// The rule a resumed `create_connector` follows: an existing row gets a fresh secret.
async fn issue_connector(
    credentials: &dyn CredentialStore,
    mutation_id: MutationId,
    connector: NewConnector,
) -> Result<IssuedConnector, ApiError> {
    match credentials.create_connector(mutation_id, connector).await? {
        ConnectorIssue::Issued(issued) => Ok(issued),
        ConnectorIssue::Existing(existing) => {
            credentials
                .rotate_connector_secret(existing.connector_id)
                .await
        }
    }
}

async fn credential_store_calls(
    credentials: &dyn CredentialStore,
    session: SessionRecord,
    presented_secret: &str,
) -> Result<Option<Connector>, ApiError> {
    credentials.installation_identity().await?;
    let stored = credentials.insert_session(session).await?;
    credentials.get_session(&stored.session_id).await?;
    credentials.revoke_session(&stored.session_id).await?;
    for connector in credentials.list_connectors(true).await? {
        credentials
            .revoke_connector(connector.connector_id)
            .await?;
    }
    credentials
        .lookup_connector(secret_hash(presented_secret))
        .await
}
```

Append at the end of the file:

```rust
#[test]
fn connector_secrets_are_hashed_with_sha256_in_one_place() -> TestResult {
    assert_eq!(
        hex(&secret_hash("abc"))?,
        "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
    );
    assert_ne!(secret_hash("abc"), secret_hash("abd"));
    assert!(type_checked(&issue_connector));
    assert!(type_checked(&credential_store_calls));
    Ok(())
}
```

- [ ] **Step 2: Run and watch it fail to compile.**

```powershell
cargo test --locked -p okf-jawn-core --test ports
```

Expected: `error[E0432]: unresolved imports` naming
`okf_jawn_core::credentials::ConnectorIssue`, `NewConnector` and `secret_hash`.

- [ ] **Step 3: Implement.** Replace the whole of `crates/core/src/credentials.rs` with:

```rust
//! Connector credentials, browser sessions, and installation identity are never inferred from content.
//!
//! A connector secret is generated by the store, returned once, and kept only as its
//! `secret_hash`. `create_connector` is unique on `MutationId`: a repeated id issues nothing and
//! reports the existing connector. The caller that resumes an abandoned attempt then calls
//! `rotate_connector_secret`, which invalidates the secret nobody received and returns a new one.

use okf_jawn_contract::access::{Connector, IssuedConnector, Principal};
use okf_jawn_contract::identity::{ConnectorId, MutationId, WorkspaceId};
use sha2::{Digest as _, Sha256};

use crate::ports::PortFuture;

/// Persistent local installation owner identity (local auth mode only).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstallationIdentity {
    /// Stable subject for the local owner principal.
    pub subject: String,
    /// RFC 3339 creation time.
    pub created_at: String,
}

/// Authenticated browser session record without exposing the cookie secret.
#[derive(Debug, Clone)]
pub struct SessionRecord {
    /// Opaque session identity.
    pub session_id: String,
    /// Authenticated principal bound to the session.
    pub principal: Principal,
    /// RFC 3339 expiry.
    pub expires_at: String,
}

/// The scope of a local MCP connector to issue.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewConnector {
    /// Owner-chosen name identifying the client.
    pub label: String,
    /// Workspaces the connector may read.
    pub workspace_ids: Vec<WorkspaceId>,
    /// Also grant proposal creation; review and approval are never grantable.
    pub allow_propose: bool,
}

/// What `create_connector` found or made for one `MutationId`.
#[derive(Debug, Clone)]
pub enum ConnectorIssue {
    /// A new connector; this is the only time its secret is visible.
    Issued(IssuedConnector),
    /// A connector already exists for this `MutationId`; its secret cannot be shown again.
    Existing(Connector),
}

/// Connectors, sessions, and installation identity; storage owns the implementation.
pub trait CredentialStore: Send + Sync {
    /// Load or create the persistent local installation identity.
    fn installation_identity(&self) -> PortFuture<'_, InstallationIdentity>;
    /// Issue a local MCP connector credential.
    ///
    /// The store generates the secret from the operating system's random source, at least 32
    /// bytes, and keeps only `secret_hash` of it. Unique on `mutation_id`: a repeated id issues
    /// nothing and returns `ConnectorIssue::Existing`.
    fn create_connector(
        &self,
        mutation_id: MutationId,
        connector: NewConnector,
    ) -> PortFuture<'_, ConnectorIssue>;
    /// Replace a connector's secret; the previous secret stops authenticating at once.
    fn rotate_connector_secret(&self, connector: ConnectorId) -> PortFuture<'_, IssuedConnector>;
    /// List connector metadata without secrets, newest first.
    fn list_connectors(&self, include_revoked: bool) -> PortFuture<'_, Vec<Connector>>;
    /// Revoke a connector immediately; repeating it changes nothing.
    fn revoke_connector(&self, connector: ConnectorId) -> PortFuture<'_, Connector>;
    /// Resolve `secret_hash` of a presented secret to its connector when it is not revoked.
    fn lookup_connector(&self, hash: [u8; 32]) -> PortFuture<'_, Option<Connector>>;
    /// Persist a browser session after authentication.
    fn insert_session(&self, session: SessionRecord) -> PortFuture<'_, SessionRecord>;
    /// Resolve a session by opaque identity when unexpired.
    fn get_session<'a>(&'a self, session_id: &'a str) -> PortFuture<'a, Option<SessionRecord>>;
    /// Invalidate a browser session.
    fn revoke_session<'a>(&'a self, session_id: &'a str) -> PortFuture<'a, ()>;
}

/// The only form in which a connector secret is stored or looked up: SHA-256 of its bytes.
///
/// The store calls this when it persists a secret; the authenticating caller calls it before
/// `CredentialStore::lookup_connector`. A secret is long and random, so no salt is needed.
#[must_use]
pub fn secret_hash(secret: &str) -> [u8; 32] {
    Sha256::digest(secret.as_bytes()).into()
}
```

- [ ] **Step 4: Format and run.**

```powershell
rustfmt --edition 2024 crates/core/src/credentials.rs crates/core/tests/ports.rs
cargo test --locked -p okf-jawn-core --test ports
```

Expected: `test result: ok. 13 passed; 0 failed`.

- [ ] **Step 5: Guard and lints for this file.**

```powershell
bun test ./tests/foundation/ports.test.mjs 2>&1 | Select-String 'credentials\.rs:'
```

Expected: no line. Filtered Clippy check: no `credentials.rs` line (the
`explicit lifetimes could be elided` error at `credentials.rs:41` is cured).

- [ ] **Step 6: Commit.**

```powershell
git add crates/core/src/credentials.rs crates/core/tests/ports.rs
git commit -m @'
fix(core): let create_connector say Issued or Existing and add a rotate call.

Why: create_connector returned IssuedConnector, which always holds a secret,
so the port could not express "the row exists and its secret is gone"; it took
a wire request; no hash function was named for stored secrets; and
find_connector_by_mutation served only the removed AbandonedEffects (SPEC 11:
a connector secret is returned exactly once and never stored in plain text).
What changed: create_connector(mutation_id, NewConnector) returns
ConnectorIssue::{Issued, Existing}; rotate_connector_secret(connector_id)
returns a fresh IssuedConnector for a resumed attempt; lookup_connector takes
the hash; list_connectors takes include_revoked; credentials::secret_hash is
SHA-256 and is the single place the hash is defined;
find_connector_by_mutation is removed; the needless lifetime at line 41 is
gone.
Verified: cargo test --locked -p okf-jawn-core --test ports -> 13 passed
(SHA-256("abc") vector); the port guard reports no credentials.rs line;
filtered clippy prints no credentials.rs line.
Next: F.9 keeps plaintext sandbox tokens out of the store.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_01LoZr4ME3crLYjcTNn5GcAC
'@
```

---

### Task F.9: Sandbox capabilities — the store sees a hash, never a token or a URL

**Files:**

- Modify: `crates/core/src/sandbox.rs` (whole file, base lines 1-54)
- Modify: `crates/core/tests/ports.rs`
- Test: `crates/core/tests/ports.rs`

**What is wrong at the base commit.** `SandboxMint.url` (`sandbox.rs:21-22`) hands the store the
capability URL, which contains the plaintext token (`SandboxCapability.url` is
"`/sandbox/{capability}` … the token is the only credential and is never logged"). `mint`
returns the wire `SandboxCapability`, so the store would have to build that URL. `mint` and
`resolve` take `token_hash: &[u8]` with no function that defines the hash, so the minting and
the resolving caller could disagree. `mint` takes `Option<MutationId>` although
`create_sandbox_capability` is a read operation with no mutation.

**Decisions.** The application generates the token, calls `sandbox::token_hash`, passes only the
hash to `mint`, and builds `SandboxCapability { url, expires_at }` itself from the configured
sandbox origin. The sandbox route calls the same `token_hash` on the presented token before
`resolve`. `mint` returns `()` and takes no `MutationId`. The capability records the media type
(Task F.3: the blob store does not), so the sandbox route can send a `Content-Type` without a
second lookup.

**Interfaces:**

- Consumes: `storage::StorageScope`, `identity::{Digest, ItemId, Revision}`, `sha2::Sha256` (already a dependency of `okf-jawn-core`: `crates/core/Cargo.toml` line `sha2.workspace = true`).
- Produces:

```rust
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SandboxMint { pub item_id: ItemId, pub revision: Revision, pub object: Digest,
    pub media_type: String, pub expires_at: String }
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SandboxResolved { pub scope: StorageScope, pub item_id: ItemId, pub revision: Revision,
    pub object: Digest, pub media_type: String }

pub trait SandboxCapabilityStore: Send + Sync {
    fn mint<'a>(&'a self, scope: &'a StorageScope, hash: [u8; 32], mint: SandboxMint)
        -> PortFuture<'a, ()>;
    fn resolve(&self, hash: [u8; 32]) -> PortFuture<'_, Option<SandboxResolved>>;
}
#[must_use]
pub fn token_hash(token: &str) -> [u8; 32];
```

- Serves SPEC §11: "Uploaded HTML is hostile data until constrained by actual isolation. Use a
  sandboxed separate origin, no credentials or privileged APIs" and "No passwords, tokens,
  connector secrets or WorkOS keys appear in browser logs, vendor notes or source."

- [ ] **Step 1: Write the failing test.** In `crates/core/tests/ports.rs` add the import

```rust
use okf_jawn_core::sandbox::{SandboxCapabilityStore, SandboxMint, SandboxResolved, token_hash};
```

Add after `credential_store_calls`:

```rust
async fn sandbox_calls(
    sandbox: &dyn SandboxCapabilityStore,
    scope: &StorageScope,
    mint: SandboxMint,
    token: &str,
) -> Result<Option<SandboxResolved>, ApiError> {
    sandbox.mint(scope, token_hash(token), mint).await?;
    sandbox.resolve(token_hash(token)).await
}
```

Append at the end of the file:

```rust
#[test]
fn sandbox_tokens_are_hashed_before_they_reach_the_store() -> TestResult {
    assert_eq!(
        hex(&token_hash("abc"))?,
        "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
    );
    assert_ne!(token_hash("abc"), token_hash("abd"));
    let mint = SandboxMint {
        item_id: ItemId(Uuid::from_u128(10)),
        revision: revision('a')?,
        object: digest('b')?,
        media_type: "text/html".to_owned(),
        expires_at: "2026-10-05T00:05:00Z".to_owned(),
    };
    assert_eq!(mint.media_type, "text/html");
    assert!(type_checked(&sandbox_calls));
    Ok(())
}
```

- [ ] **Step 2: Run and watch it fail to compile.**

```powershell
cargo test --locked -p okf-jawn-core --test ports
```

Expected: `error[E0432]: unresolved import` naming `okf_jawn_core::sandbox::token_hash`.

- [ ] **Step 3: Implement.** Replace the whole of `crates/core/src/sandbox.rs` with:

```rust
//! Sandbox-origin capability tokens: stored hashed, minted and resolved by storage.
//!
//! The store never receives a plaintext token or a URL. The application generates the token,
//! passes only `token_hash` of it to `mint`, and builds the sandbox-origin URL itself. The
//! sandbox route applies the same `token_hash` to the token it is presented before `resolve`.
//! `create_sandbox_capability` is a read operation, so minting carries no `MutationId`.

use okf_jawn_contract::identity::{Digest, ItemId, Revision};
use sha2::{Digest as _, Sha256};

use crate::ports::PortFuture;
use crate::storage::StorageScope;

/// What one short-lived capability is bound to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SandboxMint {
    /// Item whose representation is served.
    pub item_id: ItemId,
    /// Exact revision.
    pub revision: Revision,
    /// Object digest served.
    pub object: Digest,
    /// Media type the sandbox route sends for the object.
    pub media_type: String,
    /// RFC 3339 expiry.
    pub expires_at: String,
}

/// The binding of an unexpired capability.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SandboxResolved {
    /// Scope the capability was minted for.
    pub scope: StorageScope,
    /// Bound item.
    pub item_id: ItemId,
    /// Bound revision.
    pub revision: Revision,
    /// Bound object.
    pub object: Digest,
    /// Media type recorded at minting.
    pub media_type: String,
}

/// Hashed sandbox capability tokens; storage owns the implementation.
pub trait SandboxCapabilityStore: Send + Sync {
    /// Record a capability under the hash of its token.
    fn mint<'a>(
        &'a self,
        scope: &'a StorageScope,
        hash: [u8; 32],
        mint: SandboxMint,
    ) -> PortFuture<'a, ()>;
    /// Resolve the hash of a presented token to its binding when unexpired.
    fn resolve(&self, hash: [u8; 32]) -> PortFuture<'_, Option<SandboxResolved>>;
}

/// The only form in which a sandbox token reaches the store: SHA-256 of its bytes.
///
/// Both the caller that mints and the route that resolves use this function, so they cannot
/// disagree about the hash.
#[must_use]
pub fn token_hash(token: &str) -> [u8; 32] {
    Sha256::digest(token.as_bytes()).into()
}
```

- [ ] **Step 4: Format and run.**

```powershell
rustfmt --edition 2024 crates/core/src/sandbox.rs crates/core/tests/ports.rs
cargo test --locked -p okf-jawn-core --test ports
```

Expected: `test result: ok. 14 passed; 0 failed`.

- [ ] **Step 5: Lints for this file.** Filtered Clippy check: no `sandbox.rs` line.

- [ ] **Step 6: Commit.**

```powershell
git add crates/core/src/sandbox.rs crates/core/tests/ports.rs
git commit -m @'
fix(core): keep the plaintext sandbox token and its URL out of the store.

Why: SandboxMint.url handed the store the capability URL, which contains the
token; mint returned the wire SandboxCapability, so the store had to build
that URL; and no function defined the hash that mint and resolve must share
(SPEC 11: no tokens in logs or source; a sandboxed separate origin with no
credentials).
What changed: sandbox::token_hash (SHA-256) is the single hash; mint(scope,
hash, SandboxMint) returns () and takes no MutationId because
create_sandbox_capability is a read; SandboxMint lost url and, with
SandboxResolved, gained media_type so the sandbox route can set Content-Type.
The application builds the URL.
Verified: cargo test --locked -p okf-jawn-core --test ports -> 14 passed;
filtered clippy prints no sandbox.rs line.
Next: F.10 removes the lookup-by-mutation methods from the row stores.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_01LoZr4ME3crLYjcTNn5GcAC
'@
```

---

### Task F.10: Confirmations, drafts, proposals — one idempotency mechanism, core parameter types

**Files:**

- Modify: `crates/core/src/confirmations.rs` (base lines 1-4 header, 50-78 trait)
- Modify: `crates/core/src/drafts.rs` (whole file, base lines 1-64)
- Modify: `crates/core/src/proposals.rs` (whole file, base lines 1-58)
- Modify: `crates/core/tests/ports.rs`
- Test: `crates/core/tests/ports.rs`, `tests/foundation/ports.test.mjs`

**What is wrong at the base commit.** Each store has an insert that is already documented as
"a reused id returns the prior row" and, beside it, a `find_*_by_mutation` method that existed
only so the deleted `AbandonedEffects` could look the row up. Two mechanisms for one rule is
how they drift apart. `ProposalStore::get` and `list` take wire requests.

**Removals in this task** (no caller exists; the only implementation is the test fixture in
package E's `crates/core/tests/dispatch.rs:684-691`, which E's plan deletes; see Deviations 8):

| Removed | Why it is safe |
| --- | --- |
| `ConfirmationStore::find_by_mutation` (`confirmations.rs:61-66`) | `create` returns the prior challenge for a repeated id |
| `DraftStore::find_by_mutation` (`drafts.rs:37-42`) | `save` returns the prior draft for a repeated id |
| `ProposalStore::find_comment_by_mutation` (`proposals.rs:52-57`) | `add_comment` returns the prior comment for a repeated id |

(`CredentialStore::find_connector_by_mutation` went in F.8, `VersionStore::find_mutation` in
F.4; `UploadStore::find_by_mutation` and `EventLog::find_by_mutation` go in F.11.)

**Decisions.**

- `DraftStore::discard` removes a row, so "returns the prior row" needs stating: the store keeps
  the removed draft's metadata under `(mutation_id, item)` and returns it when the call is
  repeated. Uniqueness for both `save` and `discard` is on `(mutation_id, item)`, because one
  Snapshot removes several drafts under a single `MutationId`.
- `DraftWrite` gains `content_digest`: the application computes it (it is the same digest a
  confirmation later binds to), so storage never has to guess how body and properties are hashed.
- `ProposalStore::get(scope, proposal: ProposalId)`; `list(scope, ProposalFilter)`.

**Interfaces:**

- Consumes: `storage::{Page, StorageScope}`, contract `proposal::{Comment, ListProposalsResponse, Proposal, ProposalStatus}`, `item::{Draft, DraftContent}`, `review::{Confirmation, ConfirmationAction, ConfirmationTarget}`.
- Produces:

```rust
// confirmations.rs — ConfirmationCreate and ConfirmationConsume are unchanged
pub trait ConfirmationStore: Send + Sync {
    fn create<'a>(&'a self, scope: &'a StorageScope, mutation_id: MutationId,
                  create: ConfirmationCreate) -> PortFuture<'a, Confirmation>;
    fn consume<'a>(&'a self, scope: &'a StorageScope, mutation_id: MutationId,
                   consume: ConfirmationConsume) -> PortFuture<'a, Confirmation>;
}

// drafts.rs
#[derive(Debug, Clone)]
pub struct DraftWrite { pub item_id: ItemId, pub editor: String, pub base_revision: Revision,
    pub body: String, pub properties: BTreeMap<String, serde_json::Value>,
    pub content_digest: Digest }
pub trait DraftStore: Send + Sync {
    fn save<'a>(&'a self, scope: &'a StorageScope, mutation_id: MutationId, draft: DraftWrite)
        -> PortFuture<'a, Draft>;
    fn get<'a>(&'a self, scope: &'a StorageScope, item: ItemId, editor: &'a str)
        -> PortFuture<'a, Option<DraftContent>>;
    fn list<'a>(&'a self, scope: &'a StorageScope, editor: &'a str) -> PortFuture<'a, Vec<Draft>>;
    fn discard<'a>(&'a self, scope: &'a StorageScope, mutation_id: MutationId, item: ItemId,
                   editor: &'a str) -> PortFuture<'a, Draft>;
}

// proposals.rs
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProposalFilter { pub status: Option<ProposalStatus>, pub page: Page }
pub trait ProposalStore: Send + Sync {
    fn insert<'a>(&'a self, scope: &'a StorageScope, mutation_id: MutationId, proposal: Proposal)
        -> PortFuture<'a, Proposal>;
    fn get<'a>(&'a self, scope: &'a StorageScope, proposal: ProposalId) -> PortFuture<'a, Proposal>;
    fn list<'a>(&'a self, scope: &'a StorageScope, filter: ProposalFilter)
        -> PortFuture<'a, ListProposalsResponse>;
    fn update<'a>(&'a self, scope: &'a StorageScope, proposal: Proposal) -> PortFuture<'a, Proposal>;
    fn add_comment<'a>(&'a self, scope: &'a StorageScope, mutation_id: MutationId,
                       proposal: ProposalId, comment: Comment) -> PortFuture<'a, Comment>;
}
```

- Serves SPEC §8: "One draft exists per (item, editor). Saving a draft never creates a revision",
  "Consuming a confirmation records the MutationId that consumed it; a retry of the same mutation
  after a crash may re-consume successfully, but a different mutation cannot", and the
  uniqueness sentence quoted in the package header.

- [ ] **Step 1: Write the failing test.** In `crates/core/tests/ports.rs` add the imports

```rust
use okf_jawn_contract::item::Draft;
use okf_jawn_contract::proposal::{Comment, Proposal, ProposalStatus};
use okf_jawn_contract::review::Confirmation;
use okf_jawn_core::confirmations::{ConfirmationConsume, ConfirmationCreate, ConfirmationStore};
use okf_jawn_core::drafts::{DraftStore, DraftWrite};
use okf_jawn_core::proposals::{ProposalFilter, ProposalStore};
```

(merge `Draft` into the existing `okf_jawn_contract::item::{…}` import, `Comment`, `Proposal`
and `ProposalStatus` into `okf_jawn_contract::proposal::{…}`, and `Confirmation` into
`okf_jawn_contract::review::{…}`).

Add after `sandbox_calls`:

```rust
async fn confirmation_calls(
    confirmations: &dyn ConfirmationStore,
    scope: &StorageScope,
    mutation_id: MutationId,
    create: ConfirmationCreate,
) -> Result<Confirmation, ApiError> {
    let issued = confirmations
        .create(scope, mutation_id, create.clone())
        .await?;
    confirmations
        .consume(
            scope,
            mutation_id,
            ConfirmationConsume {
                confirmation_id: issued.id,
                action: create.action,
                target: create.target,
                revision: create.revision,
                content_digest: create.content_digest,
                session_id: create.session_id,
                subject: create.subject,
            },
        )
        .await
}

async fn draft_calls(
    drafts: &dyn DraftStore,
    scope: &StorageScope,
    mutation_id: MutationId,
    draft: DraftWrite,
) -> Result<Draft, ApiError> {
    let item = draft.item_id;
    let editor = draft.editor.clone();
    drafts.save(scope, mutation_id, draft).await?;
    drafts.get(scope, item, &editor).await?;
    drafts.list(scope, &editor).await?;
    drafts.discard(scope, mutation_id, item, &editor).await
}

async fn proposal_calls(
    proposals: &dyn ProposalStore,
    scope: &StorageScope,
    mutation_id: MutationId,
    proposal: Proposal,
    comment: Comment,
) -> Result<Comment, ApiError> {
    let stored = proposals.insert(scope, mutation_id, proposal).await?;
    let read = proposals.get(scope, stored.id).await?;
    proposals
        .list(
            scope,
            ProposalFilter {
                status: Some(ProposalStatus::Open),
                page: Page {
                    cursor: None,
                    limit: 20,
                },
            },
        )
        .await?;
    let updated = proposals.update(scope, read).await?;
    proposals
        .add_comment(scope, mutation_id, updated.id, comment)
        .await
}
```

Append at the end of the file:

```rust
#[test]
fn row_stores_take_core_types() -> TestResult {
    let draft = DraftWrite {
        item_id: ItemId(Uuid::from_u128(10)),
        editor: "user_1".to_owned(),
        base_revision: revision('a')?,
        body: "# Draft\n".to_owned(),
        properties: BTreeMap::new(),
        content_digest: digest('d')?,
    };
    assert_eq!(draft.content_digest, digest('d')?);
    let filter = ProposalFilter {
        status: None,
        page: Page::from(PageRequest {
            cursor: None,
            limit: 10,
        }),
    };
    assert_eq!(filter.page.limit, 10);
    assert!(type_checked(&confirmation_calls));
    assert!(type_checked(&draft_calls));
    assert!(type_checked(&proposal_calls));
    Ok(())
}
```

- [ ] **Step 2: Run and watch both checks fail.**

```powershell
cargo test --locked -p okf-jawn-core --test ports
```

Expected: `error[E0432]: unresolved import` naming `okf_jawn_core::proposals::ProposalFilter`
(rustc may also report E0560 for the `content_digest` field `DraftWrite` does not have yet).

```powershell
bun test ./tests/foundation/ports.test.mjs 2>&1 | Select-String 'find_'
```

Expected: five lines remain at this point — `confirmations.rs: ConfirmationStore::find_by_mutation`,
`drafts.rs: DraftStore::find_by_mutation`, `events.rs: EventLog::find_by_mutation`,
`proposals.rs: ProposalStore::find_comment_by_mutation`, `uploads.rs: UploadStore::find_by_mutation`.

- [ ] **Step 3a: `confirmations.rs`.** Replace lines 1-4 (the `//!` header) with:

```rust
//! Session-bound human confirmation challenges; consuming one records which `MutationId` used it.
//!
//! A retry of the same `MutationId` after a crash may consume again successfully; a different
//! `MutationId` still fails as already used.
```

and replace the whole `pub trait ConfirmationStore` item with its doc comment (base lines 50-78)
with:

```rust
/// Confirmation issuance and single-use consume; storage owns the implementation.
pub trait ConfirmationStore: Send + Sync {
    /// Create a session-bound challenge and allocate its identity.
    ///
    /// Unique on `mutation_id`: a repeated id creates nothing and returns the prior challenge.
    fn create<'a>(
        &'a self,
        scope: &'a StorageScope,
        mutation_id: MutationId,
        create: ConfirmationCreate,
    ) -> PortFuture<'a, Confirmation>;
    /// Atomically consume a confirmation for `mutation_id`.
    ///
    /// Checks subject, session, action, target, revision, digest and expiry, and records which
    /// `MutationId` consumed it. Consuming again with the same `MutationId` succeeds; a
    /// different `MutationId` fails with `Conflict` as already used.
    fn consume<'a>(
        &'a self,
        scope: &'a StorageScope,
        mutation_id: MutationId,
        consume: ConfirmationConsume,
    ) -> PortFuture<'a, Confirmation>;
}
```

Leave the imports, `ConfirmationCreate` and `ConfirmationConsume` as they are.

- [ ] **Step 3b: `drafts.rs`.** Replace the whole file with:

```rust
//! Per-editor drafts: one draft per (scope, item, editor); saving never creates a revision.
//!
//! `save` and `discard` take `MutationId` and are unique on that id together with the item:
//! repeating a call writes nothing and returns what the first call returned. One Snapshot
//! removes several drafts under a single `MutationId`, which is why the item is part of the key.

use std::collections::BTreeMap;

use okf_jawn_contract::identity::{Digest, ItemId, MutationId, Revision};
use okf_jawn_contract::item::{Draft, DraftContent};

use crate::ports::PortFuture;
use crate::storage::StorageScope;

/// Draft body and properties retained for one editor.
#[derive(Debug, Clone)]
pub struct DraftWrite {
    /// Item being drafted.
    pub item_id: ItemId,
    /// Server-established editor subject.
    pub editor: String,
    /// Head revision the draft is based on.
    pub base_revision: Revision,
    /// Drafted Markdown body.
    pub body: String,
    /// Drafted complete property map.
    pub properties: BTreeMap<String, serde_json::Value>,
    /// Digest of the drafted body and properties, computed by the application.
    pub content_digest: Digest,
}

/// Draft persistence keyed per (item, editor); storage owns the implementation.
pub trait DraftStore: Send + Sync {
    /// Save or replace the editor's draft of one item and stamp the save time.
    ///
    /// A repeated `mutation_id` for the same item writes nothing and returns the draft as that
    /// first save left it.
    fn save<'a>(
        &'a self,
        scope: &'a StorageScope,
        mutation_id: MutationId,
        draft: DraftWrite,
    ) -> PortFuture<'a, Draft>;
    /// Read the editor's own draft content for one item, if any.
    fn get<'a>(
        &'a self,
        scope: &'a StorageScope,
        item: ItemId,
        editor: &'a str,
    ) -> PortFuture<'a, Option<DraftContent>>;
    /// List the editor's own drafts in a workspace, most recently saved first.
    fn list<'a>(
        &'a self,
        scope: &'a StorageScope,
        editor: &'a str,
    ) -> PortFuture<'a, Vec<Draft>>;
    /// Remove the editor's own draft of one item and return its metadata.
    ///
    /// The store remembers what it removed under the `mutation_id` and item: a repeated call
    /// removes nothing and returns that same metadata. `NotFound` when the editor has no draft
    /// of the item and this mutation removed none.
    fn discard<'a>(
        &'a self,
        scope: &'a StorageScope,
        mutation_id: MutationId,
        item: ItemId,
        editor: &'a str,
    ) -> PortFuture<'a, Draft>;
}
```

- [ ] **Step 3c: `proposals.rs`.** Replace the whole file with:

```rust
//! Proposal and comment persistence is separate from versioned content and review evidence.
//!
//! Inserting a proposal or a comment takes `MutationId`; a repeated id writes nothing and
//! returns the prior row.

use okf_jawn_contract::{
    identity::{MutationId, ProposalId},
    proposal::{Comment, ListProposalsResponse, Proposal, ProposalStatus},
};

use crate::ports::PortFuture;
use crate::storage::{Page, StorageScope};

/// Which proposals of a workspace to list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProposalFilter {
    /// Only proposals in this state; `None` lists every state.
    pub status: Option<ProposalStatus>,
    /// Bounded page.
    pub page: Page,
}

/// Durable suggested change sets and discussion; storage owns the implementation.
pub trait ProposalStore: Send + Sync {
    /// Persist a new open proposal.
    ///
    /// Unique on `mutation_id`: a repeated id inserts nothing and returns the prior proposal.
    fn insert<'a>(
        &'a self,
        scope: &'a StorageScope,
        mutation_id: MutationId,
        proposal: Proposal,
    ) -> PortFuture<'a, Proposal>;
    /// Read one proposal by identity; `NotFound` when the workspace has no such proposal.
    fn get<'a>(&'a self, scope: &'a StorageScope, proposal: ProposalId)
    -> PortFuture<'a, Proposal>;
    /// List proposals, newest first.
    fn list<'a>(
        &'a self,
        scope: &'a StorageScope,
        filter: ProposalFilter,
    ) -> PortFuture<'a, ListProposalsResponse>;
    /// Replace proposal status and retained fields after accept, decline, or conflict.
    ///
    /// Writing the same state twice changes nothing.
    fn update<'a>(
        &'a self,
        scope: &'a StorageScope,
        proposal: Proposal,
    ) -> PortFuture<'a, Proposal>;
    /// Append a discussion comment without certifying content.
    ///
    /// Unique on `mutation_id`: a repeated id inserts nothing and returns the prior comment.
    fn add_comment<'a>(
        &'a self,
        scope: &'a StorageScope,
        mutation_id: MutationId,
        proposal: ProposalId,
        comment: Comment,
    ) -> PortFuture<'a, Comment>;
}
```

- [ ] **Step 4: Format and run.**

```powershell
rustfmt --edition 2024 crates/core/src/confirmations.rs crates/core/src/drafts.rs crates/core/src/proposals.rs crates/core/tests/ports.rs
cargo test --locked -p okf-jawn-core --test ports
```

Expected: `test result: ok. 15 passed; 0 failed`.

- [ ] **Step 5: Guard and lints for these files.**

```powershell
bun test ./tests/foundation/ports.test.mjs 2>&1 | Select-String 'confirmations\.rs:|drafts\.rs:|proposals\.rs:'
```

Expected: no line. Filtered Clippy check: no line naming one of the three files.

- [ ] **Step 6: Commit.**

```powershell
git add crates/core/src/confirmations.rs crates/core/src/drafts.rs crates/core/src/proposals.rs crates/core/tests/ports.rs
git commit -m @'
refactor(core): drop lookup-by-mutation from confirmations, drafts and proposals.

Why: each store had an insert documented as returning the prior row for a
repeated MutationId and, beside it, a find_*_by_mutation method kept only for
the removed AbandonedEffects. SPEC 8 names one mechanism: the store enforces
the MutationId as unique. ProposalStore::get and list took wire requests.
What changed: ConfirmationStore::find_by_mutation, DraftStore::find_by_mutation
and ProposalStore::find_comment_by_mutation are removed. DraftStore save and
discard are unique on (mutation_id, item) and discard returns the removed
draft's metadata on a repeat; DraftWrite carries content_digest. ProposalStore
get takes ProposalId and list takes ProposalFilter { status, page }.
Verified: cargo test --locked -p okf-jawn-core --test ports -> 15 passed; the
port guard reports no line for the three files.
Next: F.11 gives uploads and events core records.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_01LoZr4ME3crLYjcTNn5GcAC
'@
```

---

### Task F.11: Uploads and events — records that hold what the request supplied

**Files:**

- Modify: `crates/core/src/uploads.rs` (whole file, base lines 1-47)
- Modify: `crates/core/src/events.rs` (whole file, base lines 1-34)
- Modify: `crates/core/tests/ports.rs`
- Test: `crates/core/tests/ports.rs`, `tests/foundation/ports.test.mjs`

**What is wrong at the base commit.**

- `UploadStore::create(scope, mutation_id, upload: Upload)` takes the wire `Upload`
  (`uploads.rs:18-23`), which has `id`, `upload_path`, `received_bytes`, `complete` — and none of
  what `CreateUploadRequest` supplies: `filename`, `relative_path`, `size`, `sha256`. The store
  cannot finalize "against expected length/hash" and the import that follows cannot "preserve
  supplied filenames and paths" (SPEC §4, §5). The wire `upload_path` is a server route the store
  has no business building.
- `EventLog::append(scope, mutation_id, event: Event)` takes the wire `Event` whose `id` is the
  cursor the store itself must allocate, and requires a `MutationId` even for a job-progress
  notification, which repeats under one mutation.
- Both keep a `find_by_mutation`; `EventLog::list` takes a wire request.

**Decisions.**

- `NewUpload` holds the request's inputs plus who supplied them; `UploadRecord` is what the store
  returns, with the object identity once complete. The application maps a record to the wire
  `Upload` and builds `upload_path`.
- Events are notifications, not content (SPEC §2 lists the durable records; events are not among
  the stores SPEC §8 requires to be unique). `append` takes `Option<MutationId>`: with `Some`, a
  second identical notification (same kind, revision, item and job) under that mutation is not
  appended and the prior event is returned; with `None` (job progress) it always appends.
- `UploadStore::find_by_mutation` and `EventLog::find_by_mutation` are removed.

**Interfaces:**

- Consumes: `storage::{ByteReader, ObjectInfo, Page, Provenance, StorageScope}`, contract `events::{Event, EventKind, ListEventsResponse}`, ids.
- Produces:

```rust
// uploads.rs
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewUpload { pub filename: String, pub relative_path: String, pub expected_size: u64,
    pub expected_sha256: Option<Digest>, pub supplied_by: Provenance }
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UploadRecord { pub id: UploadId, pub filename: String, pub relative_path: String,
    pub expected_size: u64, pub expected_sha256: Option<Digest>, pub supplied_by: Provenance,
    pub created_at: String, pub received_bytes: u64, pub object: Option<ObjectInfo> }
pub trait UploadStore: Send + Sync {
    fn create<'a>(&'a self, scope: &'a StorageScope, mutation_id: MutationId, upload: NewUpload)
        -> PortFuture<'a, UploadRecord>;
    fn get<'a>(&'a self, scope: &'a StorageScope, upload: UploadId) -> PortFuture<'a, UploadRecord>;
    fn put_content<'a>(&'a self, scope: &'a StorageScope, upload: UploadId, body: ByteReader,
                       limit: u64) -> PortFuture<'a, UploadRecord>;
    fn complete<'a>(&'a self, scope: &'a StorageScope, upload: UploadId, sha256: Digest)
        -> PortFuture<'a, UploadRecord>;
}

// events.rs
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewEvent { pub kind: EventKind, pub revision: Option<Revision>,
    pub item_id: Option<ItemId>, pub job_id: Option<JobId> }
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EventQuery { pub after: Option<String>, pub page: Page }
pub trait EventLog: Send + Sync {
    fn append<'a>(&'a self, scope: &'a StorageScope, mutation_id: Option<MutationId>,
                  event: NewEvent) -> PortFuture<'a, Event>;
    fn list<'a>(&'a self, scope: &'a StorageScope, query: EventQuery)
        -> PortFuture<'a, ListEventsResponse>;
}
```

- [ ] **Step 1: Write the failing test.** In `crates/core/tests/ports.rs` add the imports

```rust
use okf_jawn_contract::events::{Event, EventKind};
use okf_jawn_core::events::{EventLog, EventQuery, NewEvent};
use okf_jawn_core::uploads::{NewUpload, UploadRecord, UploadStore};
```

(merge `Event` and `EventKind` into the existing `okf_jawn_contract::events::{…}` import).

Add after `proposal_calls`:

```rust
async fn upload_calls(
    uploads: &dyn UploadStore,
    scope: &StorageScope,
    mutation_id: MutationId,
    body: ByteReader,
    sha256: Digest,
) -> Result<UploadRecord, ApiError> {
    let slot = uploads
        .create(
            scope,
            mutation_id,
            NewUpload {
                filename: "report.pdf".to_owned(),
                relative_path: "finance/2026".to_owned(),
                expected_size: 3,
                expected_sha256: Some(sha256.clone()),
                supplied_by: initiator(),
            },
        )
        .await?;
    uploads.get(scope, slot.id).await?;
    uploads
        .put_content(scope, slot.id, body, slot.expected_size)
        .await?;
    uploads.complete(scope, slot.id, sha256).await
}

async fn event_calls(
    events: &dyn EventLog,
    scope: &StorageScope,
    mutation_id: MutationId,
    job_id: JobId,
) -> Result<Event, ApiError> {
    let progress = NewEvent {
        kind: EventKind::JobUpdated,
        revision: None,
        item_id: None,
        job_id: Some(job_id),
    };
    events.append(scope, None, progress.clone()).await?;
    events
        .list(
            scope,
            EventQuery {
                after: None,
                page: Page {
                    cursor: None,
                    limit: 100,
                },
            },
        )
        .await?;
    events.append(scope, Some(mutation_id), progress).await
}
```

Append at the end of the file:

```rust
#[test]
fn an_upload_slot_keeps_what_the_request_supplied() -> TestResult {
    let slot = NewUpload {
        filename: "report.pdf".to_owned(),
        relative_path: "finance/2026".to_owned(),
        expected_size: 3,
        expected_sha256: Some(digest('e')?),
        supplied_by: initiator(),
    };
    assert_eq!(slot.filename, "report.pdf");
    assert_eq!(slot.relative_path, "finance/2026");
    assert_eq!(slot.supplied_by, initiator());
    assert!(type_checked(&upload_calls));
    assert!(type_checked(&event_calls));
    Ok(())
}
```

- [ ] **Step 2: Run and watch it fail to compile.**

```powershell
cargo test --locked -p okf-jawn-core --test ports
```

Expected: `error[E0432]: unresolved imports` naming `okf_jawn_core::uploads::NewUpload`,
`okf_jawn_core::uploads::UploadRecord`, `okf_jawn_core::events::EventQuery` and
`okf_jawn_core::events::NewEvent`.

- [ ] **Step 3a: `uploads.rs`.** Replace the whole file with:

```rust
//! Upload slots track authenticated source occurrences before conversion begins.
//!
//! A slot records what the caller supplied (filename, relative folder, expected length and
//! hash) so the import that follows preserves the occurrence. Opening a slot takes
//! `MutationId`; a repeated id opens nothing and returns the prior slot.

use okf_jawn_contract::identity::{Digest, MutationId, UploadId};

use crate::ports::PortFuture;
use crate::storage::{ByteReader, ObjectInfo, Provenance, StorageScope};

/// One source occurrence about to be uploaded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewUpload {
    /// Original filename, preserved exactly.
    pub filename: String,
    /// Supplied folder context, preserved exactly; empty when none was supplied.
    pub relative_path: String,
    /// Byte count the caller announced.
    pub expected_size: u64,
    /// Content hash the caller announced, when it knew one.
    pub expected_sha256: Option<Digest>,
    /// Who supplies the bytes.
    pub supplied_by: Provenance,
}

/// A durable upload slot and how far it has come.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UploadRecord {
    /// Slot identity, allocated by the store.
    pub id: UploadId,
    /// Original filename.
    pub filename: String,
    /// Supplied folder context.
    pub relative_path: String,
    /// Byte count the caller announced.
    pub expected_size: u64,
    /// Content hash the caller announced, when it knew one.
    pub expected_sha256: Option<Digest>,
    /// Who supplies the bytes.
    pub supplied_by: Provenance,
    /// RFC 3339 time the slot was opened.
    pub created_at: String,
    /// Bytes durably received so far.
    pub received_bytes: u64,
    /// Retained object identity; present exactly when the upload is complete.
    pub object: Option<ObjectInfo>,
}

/// Durable upload registration and completion; storage owns the implementation.
pub trait UploadStore: Send + Sync {
    /// Open a slot before authenticated binary bytes arrive.
    ///
    /// Unique on `mutation_id`: a repeated id opens nothing and returns the prior slot.
    fn create<'a>(
        &'a self,
        scope: &'a StorageScope,
        mutation_id: MutationId,
        upload: NewUpload,
    ) -> PortFuture<'a, UploadRecord>;
    /// Read one slot; `NotFound` when the workspace has no such slot.
    fn get<'a>(
        &'a self,
        scope: &'a StorageScope,
        upload: UploadId,
    ) -> PortFuture<'a, UploadRecord>;
    /// Append authenticated bytes to a slot.
    ///
    /// Fails with `TooLarge` when the slot would exceed `limit` or its announced size.
    fn put_content<'a>(
        &'a self,
        scope: &'a StorageScope,
        upload: UploadId,
        body: ByteReader,
        limit: u64,
    ) -> PortFuture<'a, UploadRecord>;
    /// Verify the received bytes against the announced length and `sha256`, and retain them.
    ///
    /// Fails with `Conflict` on a length or hash mismatch and retains nothing. Completing a
    /// complete slot with the same `sha256` changes nothing and returns the same record.
    fn complete<'a>(
        &'a self,
        scope: &'a StorageScope,
        upload: UploadId,
        sha256: Digest,
    ) -> PortFuture<'a, UploadRecord>;
}
```

- [ ] **Step 3b: `events.rs`.** Replace the whole file with:

```rust
//! Resumable workspace notifications are projections, not canonical document content.
//!
//! A notification is a hint to read again; it is appended after the durable change it projects.
//! The store allocates the monotonic cursor.

use okf_jawn_contract::events::{Event, EventKind, ListEventsResponse};
use okf_jawn_contract::identity::{ItemId, JobId, MutationId, Revision};

use crate::ports::PortFuture;
use crate::storage::{Page, StorageScope};

/// One notification to append.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewEvent {
    /// What changed.
    pub kind: EventKind,
    /// Content revision, when the change produced one.
    pub revision: Option<Revision>,
    /// Affected item.
    pub item_id: Option<ItemId>,
    /// Affected job.
    pub job_id: Option<JobId>,
}

/// Which notifications to read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EventQuery {
    /// Last event cursor the reader has seen; `None` reads from the start.
    pub after: Option<String>,
    /// Bounded page.
    pub page: Page,
}

/// Append-only event log for live notifications; storage owns the implementation.
pub trait EventLog: Send + Sync {
    /// Append one notification after the durable change it projects.
    ///
    /// With a `mutation_id`, an identical notification already appended under that mutation is
    /// not appended again and the prior event is returned. Without one, as for job progress, the
    /// notification is always appended.
    fn append<'a>(
        &'a self,
        scope: &'a StorageScope,
        mutation_id: Option<MutationId>,
        event: NewEvent,
    ) -> PortFuture<'a, Event>;
    /// Read notifications after a cursor, oldest first, with bounded pagination.
    fn list<'a>(
        &'a self,
        scope: &'a StorageScope,
        query: EventQuery,
    ) -> PortFuture<'a, ListEventsResponse>;
}
```

- [ ] **Step 4: Format and run.**

```powershell
rustfmt --edition 2024 crates/core/src/uploads.rs crates/core/src/events.rs crates/core/tests/ports.rs
cargo test --locked -p okf-jawn-core --test ports
```

Expected: `test result: ok. 16 passed; 0 failed`.

- [ ] **Step 5: Guard and lints for these files.**

```powershell
bun test ./tests/foundation/ports.test.mjs 2>&1 | Select-String 'find_|uploads\.rs:|events\.rs:'
```

Expected: no line; the case `no store keeps a lookup by mutation beside its idempotent insert`
now passes. Filtered Clippy check: no `uploads.rs` or `events.rs` line.

- [ ] **Step 6: Commit.**

```powershell
git add crates/core/src/uploads.rs crates/core/src/events.rs crates/core/tests/ports.rs
git commit -m @'
fix(core): store what an upload request supplied and let the event log allocate cursors.

Why: UploadStore::create took the wire Upload, which has no filename, folder,
expected size or hash, so the slot could not be finalized against expected
length and hash and the import could not preserve the occurrence (SPEC 4:
preserve supplied filenames and paths; SPEC 5: upload slots are finalized
against expected length/hash). EventLog::append took the wire Event with a
cursor only the store can allocate and demanded a MutationId for repeated job
progress. Both kept a lookup by mutation and EventLog::list took a wire request.
What changed: NewUpload and UploadRecord carry filename, relative_path,
expected size and hash, supplier, received bytes and the retained object;
every UploadStore method returns UploadRecord. EventLog::append takes
Option<MutationId> and NewEvent and returns the stored Event; list takes
EventQuery. UploadStore::find_by_mutation and EventLog::find_by_mutation are
removed.
Verified: cargo test --locked -p okf-jawn-core --test ports -> 16 passed; the
guard case about lookups by mutation passes.
Next: F.12 gives SearchIndex core queries.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_01LoZr4ME3crLYjcTNn5GcAC
'@
```

---

### Task F.12: `SearchIndex` — one resolved revision, inside a core query

**Files:**

- Modify: `crates/core/src/search.rs` (whole file, base lines 1-49)
- Modify: `crates/core/tests/ports.rs`
- Test: `crates/core/tests/ports.rs`, `tests/foundation/ports.test.mjs`

**What is wrong at the base commit.** `search`, `links` and `graph` each take a resolved
`revision: Revision` **and** a wire request that carries its own `at: At`
(`search.rs:27-46`). Two revisions in one call: an implementation must ignore one, and nothing
says which. The wire requests also carry `workspace_id`, which duplicates `scope`.

**Interfaces:**

- Consumes: `storage::{Page, StorageScope}`, contract `search::{GetGraphResponse, GetLinksResponse, LinkDirection, SearchResponse}`, ids.
- Produces:

```rust
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchQuery { pub revision: Revision, pub text: String,
    pub folder: Option<WorkspacePath>, pub include_archived: bool, pub page: Page }
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LinkQuery { pub revision: Revision, pub item_id: ItemId,
    pub direction: LinkDirection, pub page: Page }
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GraphQuery { pub revision: Revision, pub folder: Option<WorkspacePath>, pub max_nodes: u32 }

pub trait SearchIndex: Send + Sync {
    fn index_revision<'a>(&'a self, scope: &'a StorageScope, revision: Revision) -> PortFuture<'a, ()>;
    fn search<'a>(&'a self, scope: &'a StorageScope, query: SearchQuery) -> PortFuture<'a, SearchResponse>;
    fn links<'a>(&'a self, scope: &'a StorageScope, query: LinkQuery) -> PortFuture<'a, GetLinksResponse>;
    fn graph<'a>(&'a self, scope: &'a StorageScope, query: GraphQuery) -> PortFuture<'a, GetGraphResponse>;
    fn rebuild<'a>(&'a self, scope: &'a StorageScope, head: Revision) -> PortFuture<'a, ()>;
}
```

- How storage implements it: SQLite FTS5 tables through `rusqlite` (`features = ["bundled"]`)
  keyed by tenant, workspace and revision; links and backlinks from okf-core's
  `Bundle::links_from(&ConceptId) -> &[ResolvedLink]` and `Bundle::backlinks(&ConceptId) -> &[ConceptId]`
  over the committed tree (never a draft). `rebuild` deletes and refills only these tables.
- Serves SPEC §7: "Search, link and graph data are a rebuildable derived index behind core's
  SearchIndex port. … The Application owns authorization, workspace scoping, revision resolution"
  and "rebuilding search never removes durable application records."

- [ ] **Step 1: Write the failing test.** In `crates/core/tests/ports.rs` add the imports

```rust
use okf_jawn_contract::search::{GetGraphResponse, LinkDirection};
use okf_jawn_core::search::{GraphQuery, LinkQuery, SearchIndex, SearchQuery};
```

Add after `event_calls`:

```rust
async fn search_index_calls(
    index: &dyn SearchIndex,
    scope: &StorageScope,
    head: Revision,
    item: ItemId,
) -> Result<GetGraphResponse, ApiError> {
    index.index_revision(scope, head.clone()).await?;
    index
        .search(
            scope,
            SearchQuery {
                revision: head.clone(),
                text: "quarterly revenue".to_owned(),
                folder: None,
                include_archived: false,
                page: Page {
                    cursor: None,
                    limit: 20,
                },
            },
        )
        .await?;
    index
        .links(
            scope,
            LinkQuery {
                revision: head.clone(),
                item_id: item,
                direction: LinkDirection::Both,
                page: Page {
                    cursor: None,
                    limit: 20,
                },
            },
        )
        .await?;
    index.rebuild(scope, head.clone()).await?;
    index
        .graph(
            scope,
            GraphQuery {
                revision: head,
                folder: None,
                max_nodes: 200,
            },
        )
        .await
}
```

Append at the end of the file:

```rust
#[test]
fn a_search_query_names_exactly_one_revision() -> TestResult {
    let query = SearchQuery {
        revision: revision('a')?,
        text: "quarterly revenue".to_owned(),
        folder: Some(WorkspacePath::try_from("finance".to_owned())?),
        include_archived: false,
        page: Page {
            cursor: None,
            limit: 20,
        },
    };
    assert_eq!(query.revision, revision('a')?);
    assert_eq!(
        query.folder.as_ref().map(WorkspacePath::as_str),
        Some("finance")
    );
    assert!(type_checked(&search_index_calls));
    Ok(())
}
```

- [ ] **Step 2: Run and watch it fail to compile.**

```powershell
cargo test --locked -p okf-jawn-core --test ports
```

Expected: `error[E0432]: unresolved imports` naming `okf_jawn_core::search::GraphQuery`,
`LinkQuery` and `SearchQuery`.

- [ ] **Step 3: Implement.** Replace the whole of `crates/core/src/search.rs` with:

```rust
//! Derived search and link indexes over committed workspace content.
//!
//! Everything behind this port is rebuildable from retained Git content. Rebuilding never
//! touches jobs, reviews, or receipts, which live only in `RecordStore` and cannot be rebuilt.
//! Every query names exactly one resolved `Revision`; drafts are never indexed.

use okf_jawn_contract::identity::{ItemId, Revision, WorkspacePath};
use okf_jawn_contract::search::{
    GetGraphResponse, GetLinksResponse, LinkDirection, SearchResponse,
};

use crate::ports::PortFuture;
use crate::storage::{Page, StorageScope};

/// A full-text query over one indexed revision.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchQuery {
    /// Revision to search.
    pub revision: Revision,
    /// Search expression typed by the caller; never executable SQL.
    pub text: String,
    /// Limit results to this folder and the folders beneath it.
    pub folder: Option<WorkspacePath>,
    /// Include archived and deprecated items.
    pub include_archived: bool,
    /// Bounded page.
    pub page: Page,
}

/// The links of one item at one indexed revision.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LinkQuery {
    /// Revision to read.
    pub revision: Revision,
    /// Item whose links are read.
    pub item_id: ItemId,
    /// Outgoing links, backlinks, or both.
    pub direction: LinkDirection,
    /// Bounded page.
    pub page: Page,
}

/// A bounded graph projection of one indexed revision.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GraphQuery {
    /// Revision to read.
    pub revision: Revision,
    /// Limit the graph to this folder and the folders beneath it.
    pub folder: Option<WorkspacePath>,
    /// Node limit; a result cut at this limit says so.
    pub max_nodes: u32,
}

/// Full-text, link, and graph queries; storage implements them with SQLite FTS.
///
/// The application authorizes the caller, resolves the revision once, and supplies the
/// already-scoped workspace. Implementations never widen scope and never resolve a revision.
pub trait SearchIndex: Send + Sync {
    /// Index the committed content of one revision of a workspace; indexing it again is a no-op.
    fn index_revision<'a>(
        &'a self,
        scope: &'a StorageScope,
        revision: Revision,
    ) -> PortFuture<'a, ()>;
    /// Query one indexed revision, returning `Unavailable` if that revision is not indexed.
    fn search<'a>(
        &'a self,
        scope: &'a StorageScope,
        query: SearchQuery,
    ) -> PortFuture<'a, SearchResponse>;
    /// Read outgoing links or backlinks recorded for one indexed revision.
    fn links<'a>(
        &'a self,
        scope: &'a StorageScope,
        query: LinkQuery,
    ) -> PortFuture<'a, GetLinksResponse>;
    /// Read a bounded graph projection from one indexed revision.
    fn graph<'a>(
        &'a self,
        scope: &'a StorageScope,
        query: GraphQuery,
    ) -> PortFuture<'a, GetGraphResponse>;
    /// Discard and recreate this workspace's derived index data from retained content at `head`.
    fn rebuild<'a>(&'a self, scope: &'a StorageScope, head: Revision) -> PortFuture<'a, ()>;
}
```

- [ ] **Step 4: Format and run.**

```powershell
rustfmt --edition 2024 crates/core/src/search.rs crates/core/tests/ports.rs
cargo test --locked -p okf-jawn-core --test ports
```

Expected: `test result: ok. 17 passed; 0 failed`.

- [ ] **Step 5: The whole guard is green.**

```powershell
bun test ./tests/foundation/ports.test.mjs
```

Expected: `6 pass`, `0 fail`. Filtered Clippy check: no line.

- [ ] **Step 6: Commit** (the guard file itself is committed in F.19).

```powershell
git add crates/core/src/search.rs crates/core/tests/ports.rs
git commit -m @'
fix(core): give SearchIndex one resolved revision per query.

Why: search, links and graph took a resolved Revision and also a wire request
with its own At selector, so one call named two revisions and nothing said
which the index must honour (SPEC 3: At::Latest is resolved once before the
operation reads; SPEC 7: the Application owns revision resolution).
What changed: SearchQuery, LinkQuery and GraphQuery carry the single resolved
revision, validated folder paths and a core Page; the trait takes them in
place of SearchRequest, GetLinksRequest and GetGraphRequest.
Verified: cargo test --locked -p okf-jawn-core --test ports -> 17 passed; bun
test ./tests/foundation/ports.test.mjs -> 6 pass, 0 fail.
Next: F.13 updates the Ports and module docs.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_01LoZr4ME3crLYjcTNn5GcAC
'@
```

---

### Task F.13: `Ports` and module docs say what the ports now are

**Files:**

- Modify: `crates/core/src/application/mod.rs:28-39` (doc comments only)
- Modify: `crates/core/src/lib.rs:26,28,38` (three module doc lines)
- Test: none new; `cargo test --locked -p okf-jawn-core --test ports` must stay green.

**Interfaces:**

- Consumes: every port from F.2–F.12.
- Produces: no signature change. `Ports` keeps the same seventeen fields with the same trait
  names; every trait behind them changed in place, so `Ports` already reflects the cure.
  `CandidateCheck` is deliberately **not** a field: it is core's own policy object, constructed
  by the application and passed to `versions.commit` / `versions.create_candidate` per call.

`readiness.rs` needs no change (see Deviations 13).

- [ ] **Step 1: `application/mod.rs`.** Replace the doc comment of `pub struct Ports` (base lines
28-31) with:

```rust
/// The complete set of adapters the application service coordinates.
///
/// `JobHandler` is intentionally not injected here: ingest owns the Tokio worker
/// that claims leases from `records`/`queue` and executes handlers separately.
/// `CandidateCheck` is not injected either: it is this module's own OKF conformance policy,
/// handed to `versions` with every commit and every proposal candidate.
```

and replace the doc line of the `catalog` field (base line 38,
`/// Workspace metadata and permissions.`) with:

```rust
    /// Workspace metadata keyed by tenant; the caller's permissions come from `access`.
```

- [ ] **Step 2: `lib.rs`.** Replace these three doc lines, leaving the `pub mod` lines under them
untouched:

| Base line | Old | New |
| --- | --- | --- |
| 26 | `/// Durable job and review record interfaces.` | `/// Durable job specifications, leases, reviews and receipts.` |
| 28 | `/// Durable mutation ledger and abandoned-effect lookup.` | `/// Durable mutation ledger.` |
| 38 | `/// Scoped byte and version persistence interfaces.` | `/// Blob, version and workspace-catalog ports with their core parameter types.` |

- [ ] **Step 3: Check.**

```powershell
rustfmt --check --edition 2024 crates/core/src/application/mod.rs
cargo test --locked -p okf-jawn-core --test ports
```

Expected: no diff printed; `test result: ok. 17 passed; 0 failed`. Filtered Clippy check: no
line.

- [ ] **Step 4: Commit.**

```powershell
git add crates/core/src/application/mod.rs crates/core/src/lib.rs
git commit -m @'
docs(core): describe Ports and the port modules as cured.

Why: Ports still described the catalog as holding permissions, and lib.rs
still described an abandoned-effect lookup that no longer exists.
What changed: doc comments only. Ports states that CandidateCheck is core's
own policy and is passed per commit, not injected; the catalog field says
permissions come from AccessControl; three module doc lines in lib.rs match
the modules.
Verified: cargo test --locked -p okf-jawn-core --test ports -> 17 passed;
rustfmt --check on application/mod.rs prints nothing.
Next: F.14 rewrites the storage and ingest lane briefs.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_01LoZr4ME3crLYjcTNn5GcAC
'@
```

---

### Task F.14: Lane briefs for storage and ingest match the ports

**Files:**

- Modify: `crates/storage/AGENTS.md` (whole file, base lines 1-17)
- Modify: `crates/ingest/AGENTS.md` (whole file, base lines 1-15)
- Test: none (documentation); the two existence checks in Step 3.

**Interfaces:**

- Consumes: the port signatures of F.2–F.12; the construction gates in `verification.json`
  (`storage-git-cas-sqlite`, `mutation-crash-reconcile`, `confirmation-single-use`,
  `draft-per-editor` owned by storage; `converter-worker-memory-ceiling`,
  `job-crash-against-record-store` owned by ingest).
- Produces: two briefs a lane agent can build from without reading this plan.

- [ ] **Step 1: Replace `crates/storage/AGENTS.md`** with exactly:

```markdown
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

- **A repeated `MutationId` is a no-op that returns the prior row.** This holds for `RecordStore::create_job`, `insert_review` and `insert_receipt` (when it is given one), `WorkspaceCatalog::create`, `update` and `archive`, `ConfirmationStore::create`, `DraftStore::save` and `discard` (keyed with the item), `ProposalStore::insert` and `add_comment`, `UploadStore::create`, and `CredentialStore::create_connector`, which answers `ConnectorIssue::Existing` and never a second secret. Enforce it with a unique column, not a lookup. There are no `find_*_by_mutation` methods.
- **`VersionStore::commit` is idempotent on `MutationId`.** Every commit carries an `Okf-Jawn-Mutation:` trailer. Before writing, scan `expected_head..head`; a commit with the trailer is returned with `replayed: true`. `mutation-crash-reconcile` proves it: the commit succeeds, `MutationStore::complete` is never written, the retry calls `commit` again with the same id and gets the first revision back; no second commit exists. `find_commit` exists only for Snapshot, whose expected head is not stable across attempts.
- **Stage, check, then commit.** `commit` and `create_candidate` materialize the base tree in `<data>/staging/<tenant>/<workspace>/<mutation>`, apply the `TreeEdit`s there with `okf-core` (moves, link rewrites, index regeneration), call the `CandidateCheck` they were given on that directory, and only then write the tree and the commit and move the reference. Remove the directory when the call returns, success or failure; remove anything a crash left under `<data>/staging` at startup. Never decide OKF conformance here: that is the check's job, and the check is core's.
- **No selectors, identities or wire requests.** Ports take a `StorageScope` or `TenantId`, resolved `Revision`s and core parameter types. `WorkspaceCatalog` never filters and returns `Workspace::permissions` empty.
- **Hashes are defined in core.** Store and look up connector secrets with `core::credentials::secret_hash`; sandbox tokens arrive already hashed with `core::sandbox::token_hash`. Never store a plaintext secret or token.
- **Bytes have no media type.** `BlobStore` keys objects by tenant and SHA-256; the media type lives on the source card, the asset or the sandbox capability that refers to the object.
- **Drafts are never indexed, exported or committed implicitly.** `SearchIndex` reads committed trees only.

ingest owns `JobHandler(&ClaimedJob)` and the Tokio runtime adapter; storage owns the durable stores those handlers write through, and returns the `JobSpec`, initiator and `MutationId` it was given, unchanged, on every claim.

Generator-input rule: change only this lane's authored inputs; run `gen` and commit outputs; gen-check must pass; integration owner regenerates at merge.

Do not alter shared manifests, operation declarations, generator output, or protected acceptance as a private workaround. When real code shows a port is wrong, stop and report the symbol, the SPEC sentence and the failing output to the integration owner.
```

- [ ] **Step 2: Replace `crates/ingest/AGENTS.md`** with exactly:

```markdown
# Ingestion lane

Read root AGENTS.md and SPEC.md.

**Directories:** `crates/ingest/`

**Gates** (construction gates in `verification.json` owned by this lane):

| Gate | Command or receipt |
| --- | --- |
| `converter-worker-memory-ceiling` | `cargo test -p okf-jawn-ingest --features runtime` |
| `job-crash-against-record-store` | `.artifacts/qualification/` or a construction receipt for import restart: kill and restart against the real `RecordStore`; no duplicate effect |

**Receipt:** conversion and job-handler tests; restart and idempotency evidence under `.artifacts/` (not a claim until the SPEC §12 check passes).

## Ports to implement

| Port | Defined in | Built with |
| --- | --- | --- |
| `Converter` | `core::conversion` | `docling` (`default-features = false`, `features = ["pdf"]`) |
| `JobHandler` | `core::jobs` | the ports below |
| `JobQueue` | `core::jobs` | Tokio, in process |

## Ports to call (implemented by storage)

`RecordStore::{claim_job, update_progress, complete_job, fail_job, pending_jobs, expire_leases}`, `UploadStore::get`, `BlobStore::{put, materialize}`, `VersionStore::{head, show, commit}`, `SearchIndex::{index_revision, rebuild}`, `EventLog::append`.

## Rules

- **A claim is all a handler gets and all it needs.** `ClaimedJob` carries the scope, the lease, the `JobSpec` (the request's inputs with revisions resolved), the initiator and the job's `MutationId`. Do not read request state from anywhere else.
- **A repeated `MutationId` is a no-op that returns the prior row.** A handler may run twice for one job. Write every commit with `CommitChanges { mutation_id: claimed.mutation_id, author: claimed.initiator, .. }` and name new items with `core::storage::derive_item_id(claimed.mutation_id, n)`, so the second run produces the same edits and `VersionStore::commit` answers with the first revision (`replayed: true`) instead of writing again.
- **Conversion outcomes are results, not errors.** `Converter::convert` returns `Conversion` with status `Success`, `PartialSuccess`, `Unsupported` or `Failure`. Write the source card in every case (`Extraction::Converted`, `Unsupported` or `Failed`); the bytes are already retained. Return `Err` only when the worker itself is at fault.
- **Docling through its real API.** Build `DocumentConverter` from `ConversionSettings` and `ConversionInput.timeout` (`document_timeout`), and open the source with `SourceDocument::from_bytes` using the format from `ConversionInput.file_name`: a retained object's path has no extension. Keep `DoclingDocument::export_to_json` as `ConvertedDocument.structured` beside the Markdown. No Python runtime; do not enable `asr`, `fetch-images` or `vlm` without a deliberate shared change. Record real native and model asset requirements with verified sources, versions and hashes.
- **Progress and cancellation.** Call `update_progress` between units of work and stop when the returned job is cancelled. `document_timeout` is checked only between PDF pages; the worker supervisor enforces the hard time and memory bound.
- **OKF validity is not decided here.** Use `okf-core` to build concept content; the `CandidateCheck` the application supplies runs before the commit.
- iii Phase 0 qualification ended REJECTED_WITH_FALLBACK; do not re-adopt iii or add `iii-sdk` as a product dependency. Durable job truth is `RecordStore`; queue delivery is not completion. Reconcile unfinished work through `pending_jobs()` (no tenant argument).

Generator-input rule: change only this lane's authored inputs; run `gen` and commit outputs; gen-check must pass; integration owner regenerates at merge.

Do not alter shared manifests, operation declarations, generator output, or protected acceptance as a private workaround. Return concrete boundary changes to the integration owner.
```

- [ ] **Step 3: Check both briefs name only things that exist.**

```powershell
Select-String -Path crates/storage/AGENTS.md,crates/ingest/AGENTS.md -Pattern 'find_mutation|find_by_mutation|ConversionRequest|resolve\('
```

Expected: no line. (The storage brief's sentence "There are no `find_*_by_mutation` methods"
does not match these patterns.)

```powershell
foreach ($name in 'derive_item_id','secret_hash','token_hash','CandidateCheck','ConnectorIssue','JobSpec','ClaimedJob','Extraction','ConvertedDocument','update_progress','find_commit') {
  if (-not (Select-String -Path crates/core/src/*.rs -Pattern "\b$name\b" -Quiet)) { "MISSING $name" }
}
```

Expected: no output.

- [ ] **Step 4: Commit.**

```powershell
git add crates/storage/AGENTS.md crates/ingest/AGENTS.md
git commit -m @'
docs(lanes): rewrite the storage and ingest briefs to match the cured ports.

Why: the briefs listed ports by module only, named one gate each and said
"every creating store takes MutationId uniquely" without saying what a repeat
returns; the storage lane starts next and must not guess (design 2.5: lanes
sit behind briefs that match the code).
What changed: each brief has a table of ports to implement with the module
and library, the construction gates it owns from verification.json, and the
rules: a repeated MutationId is a no-op returning the prior row; commit is
idempotent on the trailer; stage, check, then commit with the staging cleanup;
no selectors, principals or wire requests; hashes defined in core; a claim is
all a handler gets; conversion outcomes are results.
Verified: every identifier the briefs name exists in crates/core/src (the
Select-String loop prints nothing).
Next: F.15 puts a MutationId on job retry and cancel.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_01LoZr4ME3crLYjcTNn5GcAC
'@
```

---

### Task F.15: `retry_job` and `cancel_job` take a `MutationId`

**Files:**

- Modify: `crates/core/src/jobs.rs` — the `cancel_job` and `retry_job` items of `RecordStore`
- Modify: `crates/core/tests/ports.rs` — two lines of `record_store_calls`
- Modify: `tests/foundation/ports.test.mjs` (still untracked) — two new cases
- Modify: `crates/storage/AGENTS.md` — one sentence of the first rule
- Test: `crates/core/tests/ports.rs`, `tests/foundation/ports.test.mjs`

**What is wrong after F.7.** `retry_job(scope, job)` and `cancel_job(scope, job)` take no write
identity. `RetryJobRequest` and `CancelJobRequest` both carry an `idempotency_key`, so dispatch
gives each a `MutationId`; a resumed `retry_job` whose first attempt already queued the job
would queue it a second time if the job had meanwhile run and failed again. "Repeating it
changes nothing" was true only while the job's state had not moved.

**Decision.** Both take `mutation_id` and are unique on it: a repeated id makes no second
transition and returns the job row as it stands. Storage records the id in the same SQLite
transaction as the state change (a `job_transitions` table with a unique `mutation_id`, inserted
with `ON CONFLICT DO NOTHING`; the `UPDATE` runs only when the insert took effect).

The guard gains a rule so this cannot regress: a fixed list of every port method that creates a
durable row, or moves one to a new state, each with the word its signature must contain.

**Interfaces:**

- Consumes: `storage::StorageScope`, `identity::{JobId, MutationId}`, `import::Job`.
- Produces:

```rust
pub trait RecordStore: Send + Sync {
    // … unchanged methods …
    fn cancel_job<'a>(&'a self, scope: &'a StorageScope, mutation_id: MutationId, job: JobId)
        -> PortFuture<'a, Job>;
    fn retry_job<'a>(&'a self, scope: &'a StorageScope, mutation_id: MutationId, job: JobId)
        -> PortFuture<'a, Job>;
}
```

- Serves SPEC §5 "status, attempts, cancellation, retry and reconciliation survive interruption",
  §4 "An explicit retry must not create a duplicate occurrence", and the §8 uniqueness sentence.

- [ ] **Step 1: Extend the guard.** In `tests/foundation/ports.test.mjs` insert, directly before
the line ``const sample = ` ``:

```js
/** Every method that creates a durable row, or moves one to a new state, takes the write identity. */
const writes = new Map([
  ['confirmations.rs', [['ConfirmationStore', 'create', 'MutationId'], ['ConfirmationStore', 'consume', 'MutationId']]],
  ['credentials.rs', [['CredentialStore', 'create_connector', 'MutationId']]],
  ['drafts.rs', [['DraftStore', 'save', 'MutationId'], ['DraftStore', 'discard', 'MutationId']]],
  ['jobs.rs', [
    ['RecordStore', 'create_job', 'NewJob'],
    ['RecordStore', 'retry_job', 'MutationId'],
    ['RecordStore', 'cancel_job', 'MutationId'],
    ['RecordStore', 'record_artifact', 'MutationId'],
    ['RecordStore', 'insert_review', 'MutationId'],
  ]],
  ['proposals.rs', [['ProposalStore', 'insert', 'MutationId'], ['ProposalStore', 'add_comment', 'MutationId']]],
  ['storage.rs', [
    ['VersionStore', 'commit', 'CommitChanges'],
    ['VersionStore', 'create_candidate', 'CandidateChanges'],
    ['VersionStore', 'promote_candidate', 'Promotion'],
    ['WorkspaceCatalog', 'create', 'MutationId'],
    ['WorkspaceCatalog', 'update', 'MutationId'],
    ['WorkspaceCatalog', 'archive', 'MutationId'],
  ]],
  ['uploads.rs', [['UploadStore', 'create', 'MutationId']]],
]);

function writesWithoutIdentity(file, rust, expected) {
  const signatures = traitSignatures(rust);
  const out = [];
  for (const [trait, name, token] of expected) {
    const signature = signatures.find(candidate => candidate.trait === trait && candidate.name === name);
    if (!signature) out.push(`${file}: ${trait}::${name} is missing`);
    else if (!signature.text.split(/[^A-Za-z0-9_]+/).includes(token)) out.push(`${file}: ${trait}::${name} does not take ${token}`);
  }
  return out;
}
```

and append at the end of the file:

```js
test('a write that lacks the write identity, or is missing, is reported', () => {
  const expected = [['Sample', 'find_commit', 'MutationId'], ['Sample', 'head', 'MutationId'], ['Sample', 'record', 'MutationId']];
  assert.deepEqual(writesWithoutIdentity('sample.rs', sample, expected), [
    'sample.rs: Sample::head does not take MutationId',
    'sample.rs: Sample::record is missing',
  ]);
});

test('every method that writes a durable row takes the write identity', async () => {
  const found = [];
  for (const [file, expected] of writes) {
    found.push(...writesWithoutIdentity(file, await readFile(join(source, file), 'utf8'), expected));
  }
  assert.deepEqual(found, []);
});
```

- [ ] **Step 2: Change the test.** In `crates/core/tests/ports.rs`, inside `record_store_calls`,
replace

```rust
    records.cancel_job(scope, job.id).await?;
    records.retry_job(scope, job.id).await?;
```

with

```rust
    records.cancel_job(scope, mutation_id, job.id).await?;
    records.retry_job(scope, mutation_id, job.id).await?;
```

- [ ] **Step 3: Run and watch both checks fail.**

```powershell
cargo test --locked -p okf-jawn-core --test ports
```

Expected: `error[E0061]: this method takes 2 arguments but 3 arguments were supplied`, twice.

```powershell
bun test ./tests/foundation/ports.test.mjs
```

Expected: `7 pass`, `1 fail`. The failing case is
`every method that writes a durable row takes the write identity`, with exactly (this list was
produced by running the extended guard against the port files as F.12 leaves them):

```text
jobs.rs: RecordStore::retry_job does not take MutationId
jobs.rs: RecordStore::cancel_job does not take MutationId
jobs.rs: RecordStore::record_artifact is missing
```

- [ ] **Step 4: Implement.** In `crates/core/src/jobs.rs` replace the `cancel_job` and
`retry_job` items of `RecordStore` (each with its doc comment) with:

```rust
    /// Cancel pending or running work while retaining recorded state.
    ///
    /// Unique on `mutation_id`: a repeated id makes no second transition and returns the job
    /// row as it stands.
    fn cancel_job<'a>(
        &'a self,
        scope: &'a StorageScope,
        mutation_id: MutationId,
        job: JobId,
    ) -> PortFuture<'a, Job>;
    /// Queue a failed job again under the same durable identity.
    ///
    /// Unique on `mutation_id`: a repeated id does not queue the job a second time, even when
    /// the job has run and failed again since, and returns the job row as it stands.
    fn retry_job<'a>(
        &'a self,
        scope: &'a StorageScope,
        mutation_id: MutationId,
        job: JobId,
    ) -> PortFuture<'a, Job>;
```

- [ ] **Step 5: Update the storage brief.** In `crates/storage/AGENTS.md` replace

```text
This holds for `RecordStore::create_job`, `insert_review` and `insert_receipt` (when it is given one),
```

with

```text
This holds for `RecordStore::create_job`, `retry_job`, `cancel_job`, `insert_review` and `insert_receipt` (when it is given one),
```

(the sentence is part of one long line; change only these words).

- [ ] **Step 6: Format and run.**

```powershell
rustfmt --edition 2024 crates/core/src/jobs.rs crates/core/tests/ports.rs
cargo test --locked -p okf-jawn-core --test ports
bun test ./tests/foundation/ports.test.mjs
```

Expected: `test result: ok. 17 passed; 0 failed`; then `7 pass`, `1 fail` with the single line
`jobs.rs: RecordStore::record_artifact is missing` (Task F.17 clears it). Filtered Clippy check:
no `jobs.rs` line.

- [ ] **Step 7: Commit.**

```powershell
git add crates/core/src/jobs.rs crates/core/tests/ports.rs crates/storage/AGENTS.md
git commit -m @'
fix(core): make job retry and cancel unique on their MutationId.

Why: retry_job and cancel_job took no write identity although both requests
carry an idempotency key. A resumed retry whose first attempt had already
queued the job would queue it again once the job had run and failed (SPEC 4:
an explicit retry must not create a duplicate occurrence; SPEC 8: a retry
after an abandoned lease never duplicates an effect).
What changed: RecordStore::cancel_job and retry_job take mutation_id; a
repeated id makes no second transition and returns the job row as it stands.
The storage brief lists both among the methods a repeated MutationId leaves
unchanged. The untracked signature guard gains a list of every writing port
method and the word its signature must contain.
Verified: cargo test --locked -p okf-jawn-core --test ports -> 17 passed; the
guard's new case reports only RecordStore::record_artifact as missing, which
F.17 adds.
Next: F.16 lets a proposal's discussion be listed.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_01LoZr4ME3crLYjcTNn5GcAC
'@
```

---

### Task F.16: `ProposalStore::list_comments`

**Files:**

- Modify: `crates/core/src/proposals.rs` — add `CommentPage` after `ProposalFilter`; add `list_comments` as the last method of `ProposalStore`
- Modify: `crates/core/tests/ports.rs`
- Test: `crates/core/tests/ports.rs`

**What is wrong after F.10.** `add_comment` writes a comment and nothing can read one back:
the proposal view cannot show the discussion it lets people add to.

**Interfaces:**

- Consumes: `storage::{Page, StorageScope}`, `identity::ProposalId`, `proposal::Comment`.
- Produces:

```rust
#[derive(Debug, Clone)]
pub struct CommentPage { pub items: Vec<Comment>, pub next_cursor: Option<String> }

pub trait ProposalStore: Send + Sync {
    // … unchanged methods …
    fn list_comments<'a>(&'a self, scope: &'a StorageScope, proposal: ProposalId, page: Page)
        -> PortFuture<'a, CommentPage>;
}
```

  `Page` is the core page parameter from F.2; the result is a core type because the contract has
  no response type for a comment listing (see Deviations 10).
- How storage implements it: the `comments` table `add_comment` already writes, read with
  `WHERE proposal_id = ? AND rowid > ? ORDER BY rowid LIMIT ?`; the cursor is the last row id.
- Serves SPEC §1 "proposals, review, Attention" in the human experience and §8 "An authorized
  person can accept or decline the exact proposal shown."

- [ ] **Step 1: Write the failing test.** In `crates/core/tests/ports.rs` change the proposals
import to

```rust
use okf_jawn_core::proposals::{CommentPage, ProposalFilter, ProposalStore};
```

Replace the whole `proposal_calls` function with:

```rust
async fn proposal_calls(
    proposals: &dyn ProposalStore,
    scope: &StorageScope,
    mutation_id: MutationId,
    proposal: Proposal,
    comment: Comment,
) -> Result<CommentPage, ApiError> {
    let stored = proposals.insert(scope, mutation_id, proposal).await?;
    let read = proposals.get(scope, stored.id).await?;
    proposals
        .list(
            scope,
            ProposalFilter {
                status: Some(ProposalStatus::Open),
                page: Page {
                    cursor: None,
                    limit: 20,
                },
            },
        )
        .await?;
    let updated = proposals.update(scope, read).await?;
    proposals
        .add_comment(scope, mutation_id, updated.id, comment)
        .await?;
    let page = Page {
        cursor: None,
        limit: 50,
    };
    proposals.list_comments(scope, updated.id, page).await
}
```

Append at the end of the file:

```rust
#[test]
fn a_page_of_comments_keeps_its_order_and_cursor() {
    let page = CommentPage {
        items: vec![Comment {
            id: "c1".to_owned(),
            author: "user_1".to_owned(),
            text: "Why this figure?".to_owned(),
            created_at: "2026-10-05T00:00:00Z".to_owned(),
        }],
        next_cursor: Some("after-c1".to_owned()),
    };
    assert_eq!(
        page.items.first().map(|comment| comment.id.as_str()),
        Some("c1")
    );
    assert_eq!(page.next_cursor.as_deref(), Some("after-c1"));
    assert!(type_checked(&proposal_calls));
}
```

- [ ] **Step 2: Run and watch it fail to compile.**

```powershell
cargo test --locked -p okf-jawn-core --test ports
```

Expected: `error[E0432]: unresolved import` naming `okf_jawn_core::proposals::CommentPage`.

- [ ] **Step 3: Implement.** In `crates/core/src/proposals.rs` insert directly after the closing
brace of `pub struct ProposalFilter`:

```rust

/// One page of a proposal's discussion, oldest first.
#[derive(Debug, Clone)]
pub struct CommentPage {
    /// Comments on this page.
    pub items: Vec<Comment>,
    /// Continuation cursor, when more comments remain.
    pub next_cursor: Option<String>,
}
```

and add as the last method of `pub trait ProposalStore`, after `add_comment`:

```rust
    /// List a proposal's discussion, oldest first; `NotFound` when the workspace has no such
    /// proposal.
    fn list_comments<'a>(
        &'a self,
        scope: &'a StorageScope,
        proposal: ProposalId,
        page: Page,
    ) -> PortFuture<'a, CommentPage>;
```

- [ ] **Step 4: Format and run.**

```powershell
rustfmt --edition 2024 crates/core/src/proposals.rs crates/core/tests/ports.rs
cargo test --locked -p okf-jawn-core --test ports
bun test ./tests/foundation/ports.test.mjs 2>&1 | Select-String 'proposals\.rs:'
```

Expected: `test result: ok. 18 passed; 0 failed`; the guard prints no `proposals.rs` line.
Filtered Clippy check: no `proposals.rs` line.

- [ ] **Step 5: Commit.**

```powershell
git add crates/core/src/proposals.rs crates/core/tests/ports.rs
git commit -m @'
feat(core): let a proposal's discussion be listed.

Why: ProposalStore::add_comment wrote comments that no port could read back,
so the proposal view could not show the discussion it accepts (SPEC 1 keeps
proposals and review in the human experience; SPEC 8: a person accepts or
declines the exact proposal shown).
What changed: ProposalStore::list_comments(scope, proposal, page) returns a
core CommentPage { items, next_cursor }, oldest first, using the core Page
parameter rather than a wire request.
Verified: cargo test --locked -p okf-jawn-core --test ports -> 18 passed; the
port guard reports no proposals.rs line.
Next: F.17 records artifacts.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_01LoZr4ME3crLYjcTNn5GcAC
'@
```

---

### Task F.17: Artifact records — an `ArtifactId` resolves to retained bytes

**Files:**

- Modify: `crates/core/src/jobs.rs` — the `use crate::storage` line; `JobCompletion.artifact`; new types `ArtifactKind`, `NewArtifact`, `ArtifactRecord`; two `RecordStore` methods; `complete_job` doc; `impl ArtifactRecord`; `artifact_download_path`
- Modify: `crates/core/src/application/mod.rs` (doc of the `records` field), `crates/core/src/lib.rs` (doc of `pub mod jobs`)
- Modify: `crates/storage/AGENTS.md`, `crates/ingest/AGENTS.md`
- Modify: `crates/core/tests/ports.rs`
- Test: `crates/core/tests/ports.rs`, `tests/foundation/ports.test.mjs`

**What is wrong after F.7.** `JobSpec::RestoreWorkspace { artifact_id, .. }` names an artifact,
the wire `Job.artifact` is a `DownloadArtifact { artifact_id, sha256, size, download_path }`
(`crates/contract/src/workspace.rs`), and the contract declares the transport
`download_artifact`: `GET /api/workspaces/{workspace_id}/artifacts/{artifact_id}`,
`response_media: "application/zip"`, `auth: TransportAuth::Principal`
(`crates/contract/src/transport.rs`). No port maps an `ArtifactId` to bytes, so neither the
server's download route nor a restore job can open one. `JobCompletion.artifact` takes the wire
`DownloadArtifact`, which would make an ingest handler invent the server's route string.

**Decisions.**

- The mapping lives on `RecordStore`, not a new `ArtifactStore` trait: an artifact row is
  non-rebuildable job output in the same SQLite database a backup must capture, `complete_job`
  joins it to fill `Job.artifact`, and a separate trait would add a `Ports` field and a guard
  entry for two methods.
- The bytes stay in `BlobStore` under their digest. The record is
  `ArtifactId -> { kind, object: { digest, size }, media_type, created_by_job }`. The media type
  is on the record because the blob has none (F.3) and the transport must send one.
- `record_artifact` is unique on the producing job's `MutationId` (a job produces at most one
  artifact: the wire `Job.artifact` is a single value) and the store allocates the `ArtifactId`.
- `JobCompletion.artifact` becomes `Option<ArtifactId>`. The download path is defined once, in
  core, by `artifact_download_path`; `ArtifactRecord::download` builds the wire value, and the
  record store uses it when it returns a `Job`.
- Out of scope, as ruled: the archive format and the export, backup and restore handlers.

**Interfaces:**

- Consumes: `storage::{ObjectInfo, StorageScope}`, `identity::{ArtifactId, JobId, MutationId, WorkspaceId}`, `workspace::DownloadArtifact`, `transport::TRANSPORTS` (in the test).
- Produces:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArtifactKind { Export, Backup, ViewExport }
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewArtifact { pub kind: ArtifactKind, pub object: ObjectInfo, pub media_type: String,
    pub created_by_job: JobId }
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArtifactRecord { pub id: ArtifactId, pub kind: ArtifactKind, pub object: ObjectInfo,
    pub media_type: String, pub created_by_job: JobId }
impl ArtifactRecord { #[must_use] pub fn download(&self, workspace: WorkspaceId) -> DownloadArtifact; }
#[must_use]
pub fn artifact_download_path(workspace: WorkspaceId, artifact: ArtifactId) -> String;

pub struct JobCompletion { /* … */ pub artifact: Option<ArtifactId>, /* … */ }

pub trait RecordStore: Send + Sync {
    // … unchanged methods …
    fn record_artifact<'a>(&'a self, scope: &'a StorageScope, mutation_id: MutationId,
                           artifact: NewArtifact) -> PortFuture<'a, ArtifactRecord>;
    fn get_artifact<'a>(&'a self, scope: &'a StorageScope, artifact: ArtifactId)
        -> PortFuture<'a, ArtifactRecord>;
}
```

- Callers: a producing handler runs `BlobStore::put` → `record_artifact(&claimed.scope, claimed.mutation_id, …)`
  → `complete_job(JobCompletion { artifact: Some(record.id), .. })`. The server's
  `download_artifact` route and a `restore_workspace` handler run
  `get_artifact(scope, id)` → `BlobStore::open(scope, &record.object.digest, offset, length)`.
  `get_artifact` is scoped, so an id from another workspace is `NotFound`.
- How storage implements it: an `artifacts` table in SQLite (`rusqlite`) with primary key `id`,
  a unique `mutation_id`, and the scope, kind, digest, size, media type and job id.
- Serves SPEC §12 "Portable export gathers references into an independently readable
  folder/archive. Full backup additionally preserves promised app records, retained objects and
  versions. Restore must be exercised." and §4 "Hash possession does not authorize a read."

- [ ] **Step 1: Write the failing test.** In `crates/core/tests/ports.rs` add the import

```rust
use okf_jawn_contract::transport::TRANSPORTS;
```

and add `ArtifactKind`, `ArtifactRecord` and `NewArtifact` to the `okf_jawn_core::jobs::{…}`
import.

Add after `job_runtime_calls`:

```rust
/// What an export handler and, later, a download or a restore do with an artifact.
async fn artifact_calls(
    records: &dyn RecordStore,
    blobs: &dyn BlobStore,
    claimed: &ClaimedJob,
    stored: ObjectInfo,
) -> Result<ObjectInfo, ApiError> {
    let record = records
        .record_artifact(
            &claimed.scope,
            claimed.mutation_id,
            NewArtifact {
                kind: ArtifactKind::Export,
                object: stored,
                media_type: "application/zip".to_owned(),
                created_by_job: claimed.lease.job_id,
            },
        )
        .await?;
    records
        .complete_job(JobCompletion {
            lease: claimed.lease.clone(),
            revision: None,
            item_ids: Vec::new(),
            artifact: Some(record.id),
            outputs: vec![record.object.digest.clone()],
            warnings: Vec::new(),
        })
        .await?;
    let found = records.get_artifact(&claimed.scope, record.id).await?;
    let read = blobs
        .open(&claimed.scope, &found.object.digest, 0, found.object.size)
        .await?;
    Ok(read.object)
}
```

Append at the end of the file:

```rust
#[test]
fn an_artifact_record_yields_the_wire_download_for_its_transport() -> TestResult {
    let workspace = WorkspaceId(Uuid::from_u128(1));
    let record = ArtifactRecord {
        id: ArtifactId(Uuid::from_u128(12)),
        kind: ArtifactKind::Backup,
        object: ObjectInfo {
            digest: digest('c')?,
            size: 2048,
        },
        media_type: "application/zip".to_owned(),
        created_by_job: JobId(Uuid::from_u128(7)),
    };
    let transport = TRANSPORTS
        .iter()
        .find(|transport| transport.id == "download_artifact")
        .ok_or("the download_artifact transport is not declared")?;
    let expected_path = transport
        .path
        .replace("{workspace_id}", &workspace.0.to_string())
        .replace("{artifact_id}", &record.id.0.to_string());
    let download = record.download(workspace);
    assert_eq!(download.download_path, expected_path);
    assert_eq!(download.artifact_id, record.id);
    assert_eq!(download.sha256, digest('c')?);
    assert_eq!(download.size, "2048");
    assert_eq!(transport.response_media, record.media_type);
    assert!(type_checked(&artifact_calls));
    Ok(())
}
```

- [ ] **Step 2: Run and watch it fail to compile.**

```powershell
cargo test --locked -p okf-jawn-core --test ports
```

Expected: `error[E0432]: unresolved imports` naming `okf_jawn_core::jobs::ArtifactKind`,
`ArtifactRecord` and `NewArtifact`.

- [ ] **Step 3a: `jobs.rs` imports and completion.** Replace the line
`use crate::storage::{Page, Provenance, StorageScope};` with

```rust
use crate::storage::{ObjectInfo, Page, Provenance, StorageScope};
```

and in `pub struct JobCompletion` replace the two lines of the `artifact` field with:

```rust
    /// Artifact the job produced, already recorded with `RecordStore::record_artifact`.
    pub artifact: Option<ArtifactId>,
```

- [ ] **Step 3b: `jobs.rs` types.** Insert directly after the closing brace of
`pub struct JobCompletion`:

```rust

/// What a retained artifact is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArtifactKind {
    /// A portable export of a workspace.
    Export,
    /// A full backup of content, retained objects and application records.
    Backup,
    /// An export of one View.
    ViewExport,
}

/// An artifact a job produced, recorded once its bytes are retained in the blob store.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewArtifact {
    /// What the artifact is.
    pub kind: ArtifactKind,
    /// Identity and size of the retained bytes.
    pub object: ObjectInfo,
    /// Media type the download transport sends.
    pub media_type: String,
    /// Job that produced the artifact.
    pub created_by_job: JobId,
}

/// The non-rebuildable link from an artifact identity to retained bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArtifactRecord {
    /// Artifact identity, allocated by the store.
    pub id: ArtifactId,
    /// What the artifact is.
    pub kind: ArtifactKind,
    /// Identity and size of the retained bytes; open them with `BlobStore::open`.
    pub object: ObjectInfo,
    /// Media type the download transport sends.
    pub media_type: String,
    /// Job that produced the artifact.
    pub created_by_job: JobId,
}
```

- [ ] **Step 3c: `jobs.rs` trait.** In `pub trait RecordStore` replace the doc comment of
`complete_job` (one line) with:

```rust
    /// Commit completion only for the current unexpired claim.
    ///
    /// When `completion.artifact` is set, the returned and stored job shows
    /// `ArtifactRecord::download` of that record as its artifact.
```

and insert directly after the `retry_job` item:

```rust
    /// Record the artifact a job produced and allocate its identity.
    ///
    /// The bytes are already retained in `BlobStore` under `artifact.object.digest`. Unique on
    /// `mutation_id`, the producing job's write identity: a repeated id records nothing and
    /// returns the prior record.
    fn record_artifact<'a>(
        &'a self,
        scope: &'a StorageScope,
        mutation_id: MutationId,
        artifact: NewArtifact,
    ) -> PortFuture<'a, ArtifactRecord>;
    /// Read one artifact record within workspace scope; `NotFound` when this workspace has no
    /// artifact with that identity.
    fn get_artifact<'a>(
        &'a self,
        scope: &'a StorageScope,
        artifact: ArtifactId,
    ) -> PortFuture<'a, ArtifactRecord>;
```

- [ ] **Step 3d: `jobs.rs` behaviour.** Insert directly before `impl JobSpec {`:

```rust
impl ArtifactRecord {
    /// The wire form shown on a job, with the `download_artifact` transport path.
    #[must_use]
    pub fn download(&self, workspace: WorkspaceId) -> DownloadArtifact {
        DownloadArtifact {
            artifact_id: self.id,
            sha256: self.object.digest.clone(),
            size: self.object.size.to_string(),
            download_path: artifact_download_path(workspace, self.id),
        }
    }
}

```

and append at the end of the file:

```rust

/// The application-relative path of the `download_artifact` transport for one artifact.
///
/// Defined once so the record store, the application and the server route agree.
#[must_use]
pub fn artifact_download_path(workspace: WorkspaceId, artifact: ArtifactId) -> String {
    format!(
        "/api/workspaces/{}/artifacts/{}",
        workspace.0, artifact.0
    )
}
```

- [ ] **Step 3e: Docs and briefs.**

In `crates/core/src/application/mod.rs` replace the doc line of the `records` field,
`/// Durable, non-rebuildable jobs, reviews, and receipts.`, with:

```rust
    /// Durable, non-rebuildable jobs, reviews, receipts, and artifact records.
```

In `crates/core/src/lib.rs` replace `/// Durable job specifications, leases, reviews and receipts.`
with `/// Durable job specifications, leases, reviews, receipts and artifact records.`

In `crates/storage/AGENTS.md` replace

```text
This holds for `RecordStore::create_job`, `retry_job`, `cancel_job`, `insert_review` and `insert_receipt` (when it is given one),
```

with

```text
This holds for `RecordStore::create_job`, `retry_job`, `cancel_job`, `record_artifact`, `insert_review` and `insert_receipt` (when it is given one),
```

and insert this line directly after the line that starts `- **Bytes have no media type.**`:

```markdown
- **An artifact is a record over a blob.** Export, backup and View-export bytes are retained in `BlobStore` like any other object; `RecordStore::record_artifact` maps an `ArtifactId` to `{ kind, digest, size, media type, producing job }` and `get_artifact` reads it back inside one workspace. `Job.artifact` is `ArtifactRecord::download` of that record. Artifact rows are application records: a backup includes them and rebuilding search never touches them.
```

In `crates/ingest/AGENTS.md` replace

```text
`RecordStore::{claim_job, update_progress, complete_job, fail_job, pending_jobs, expire_leases}`, `UploadStore::get`, `BlobStore::{put, materialize}`,
```

with

```text
`RecordStore::{claim_job, update_progress, record_artifact, get_artifact, complete_job, fail_job, pending_jobs, expire_leases}`, `UploadStore::get`, `BlobStore::{put, open, materialize}`,
```

- [ ] **Step 4: Format and run.**

```powershell
rustfmt --edition 2024 crates/core/src/jobs.rs crates/core/tests/ports.rs
rustfmt --check --edition 2024 crates/core/src/application/mod.rs
cargo test --locked -p okf-jawn-core --test ports
bun test ./tests/foundation/ports.test.mjs
```

Expected: `test result: ok. 19 passed; 0 failed`; `8 pass`, `0 fail`. Filtered Clippy check: no
line.

- [ ] **Step 5: Commit.**

```powershell
git add crates/core/src/jobs.rs crates/core/src/application/mod.rs crates/core/src/lib.rs crates/core/tests/ports.rs crates/storage/AGENTS.md crates/ingest/AGENTS.md
git commit -m @'
feat(core): record artifacts so an ArtifactId resolves to retained bytes.

Why: restore_workspace names an artifact_id, Job.artifact is a
DownloadArtifact, and the contract declares GET
/api/workspaces/{workspace_id}/artifacts/{artifact_id}, but no port mapped an
ArtifactId to bytes, and JobCompletion took the wire DownloadArtifact, which
made a job handler invent the server's route (SPEC 12: export, full backup and
an exercised restore; SPEC 4: hash possession does not authorize a read).
What changed: RecordStore::record_artifact(scope, mutation_id, NewArtifact)
is unique on the producing job's MutationId and returns ArtifactRecord { id,
kind, object, media_type, created_by_job }; get_artifact(scope, id) reads it
inside one workspace; the bytes stay in BlobStore. JobCompletion.artifact is
Option<ArtifactId>. artifact_download_path and ArtifactRecord::download define
the wire DownloadArtifact once. Ports docs and both lane briefs name the new
methods. The archive format and the handlers remain lane work.
Verified: cargo test --locked -p okf-jawn-core --test ports -> 19 passed (the
path equals the contract's download_artifact template); bun test
./tests/foundation/ports.test.mjs -> 8 pass, 0 fail.
Next: F.18 gives converted assets a pixel size and a caption origin.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_01LoZr4ME3crLYjcTNn5GcAC
'@
```

---

### Task F.18: `ConvertedAsset` carries pixel size and caption origin

**Files:**

- Modify: `crates/core/src/conversion.rs` — replace `ConvertedAsset`; add `PixelSize`, `AssetCaption`, `CaptionOrigin`; add `impl CaptionOrigin`
- Modify: `crates/ingest/AGENTS.md` — one rule
- Modify: `crates/core/tests/ports.rs`
- Test: `crates/core/tests/ports.rs`

**What is wrong after F.6.** The wire `MediaReference` (`crates/contract/src/read.rs`) requires
`width: u32`, `height: u32`, `caption: String` and `caption_origin: String` ("source, process,
human, or agent"). `ConvertedAsset` has `caption: Option<String>` with no origin and no
dimensions, so a read would have to decode every image again and guess whether a caption is the
document's own (SPEC §5: "Retain structured Markdown, images/captions … source locators";
§5: "AI enrichment occurs only when the user's external agent proposes it, with truthful origin
labels").

**What docling 1.93.5 supplies** (`docling-core-1.93.6/src/document.rs`):

```rust
// :133-151 — one picture in DoclingDocument.nodes
Node::Picture {
    caption: Option<String>,
    caption_href: Option<String>,
    image: Option<PictureImage>,
    classification: Option<Vec<PictureClass>>,   // "when the picture-classification enrichment ran"
    caption_parent: CaptionParent,
}
// :655-670 — "An extracted picture's raw encoded bytes plus its mimetype and pixel size"
pub struct PictureImage { pub mimetype: String, pub width: u32, pub height: u32,
                          pub data: Vec<u8>, pub dpi: u32 }
// :645-650
pub struct PictureClass { pub class_name: String, pub confidence: f32 }
```

| Core field | Docling field |
| --- | --- |
| `ConvertedAsset.media_type` | `PictureImage.mimetype` |
| file at `ConvertedAsset.path` | `PictureImage.data` ("The image file bytes, exactly as embedded") |
| `ConvertedAsset.pixel_size` | `Some(PixelSize { width: PictureImage.width, height: PictureImage.height })`; `None` for an asset that is not a raster image |
| `AssetCaption { text, origin: CaptionOrigin::Source }` | `Node::Picture.caption` when it is `Some` — the document's own caption or `alt` text |
| `AssetCaption { origin: CaptionOrigin::Process, .. }` | text the conversion generated. Docling's only generated label is `classification[..].class_name`, produced when `do_picture_classification(true)` is set; `ConversionSettings` does not expose that switch, so with today's settings the adapter emits `Source` or no caption |
| `ConvertedAsset.caption: None` | `Node::Picture.caption` is `None` |

`human` and `agent` are not conversion outcomes: they arise from corrections and proposals.

**Interfaces:**

- Consumes: `read::Selection`.
- Produces:

```rust
#[derive(Debug, Clone)]
pub struct ConvertedAsset { pub path: PathBuf, pub media_type: String, pub selection: Selection,
    pub pixel_size: Option<PixelSize>, pub caption: Option<AssetCaption> }
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PixelSize { pub width: u32, pub height: u32 }
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AssetCaption { pub text: String, pub origin: CaptionOrigin }
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CaptionOrigin { Source, Process }
impl CaptionOrigin { #[must_use] pub const fn as_str(self) -> &'static str; }   // "source" | "process"
```

  Text and origin share one struct so a caption can never be recorded without saying where it
  came from.

- [ ] **Step 1: Write the failing test.** In `crates/core/tests/ports.rs` add the import

```rust
use okf_jawn_contract::read::Selection;
```

and change the conversion import to

```rust
use okf_jawn_core::conversion::{
    AssetCaption, CaptionOrigin, ConversionInput, ConversionSettings, ConversionStatus,
    ConvertedAsset, Converter, OcrPolicy, PixelSize,
};
```

Append at the end of the file:

```rust
#[test]
fn a_converted_image_carries_its_size_and_caption_origin() {
    let asset = ConvertedAsset {
        path: PathBuf::from("out/figure-1.png"),
        media_type: "image/png".to_owned(),
        selection: Selection::All,
        pixel_size: Some(PixelSize {
            width: 640,
            height: 480,
        }),
        caption: Some(AssetCaption {
            text: "Revenue by quarter".to_owned(),
            origin: CaptionOrigin::Source,
        }),
    };
    assert_eq!(
        asset.pixel_size,
        Some(PixelSize {
            width: 640,
            height: 480,
        })
    );
    assert_eq!(
        asset
            .caption
            .as_ref()
            .map(|caption| caption.origin.as_str()),
        Some("source")
    );
    assert_eq!(CaptionOrigin::Process.as_str(), "process");
    assert_eq!(asset.media_type, "image/png");
}
```

- [ ] **Step 2: Run and watch it fail to compile.**

```powershell
cargo test --locked -p okf-jawn-core --test ports
```

Expected: `error[E0432]: unresolved imports` naming `okf_jawn_core::conversion::AssetCaption`,
`CaptionOrigin` and `PixelSize`.

- [ ] **Step 3: Implement.** In `crates/core/src/conversion.rs` replace the whole
`pub struct ConvertedAsset` item (doc comment and derive included) with:

```rust
/// One derivative generated by a converter, ready for the blob store.
#[derive(Debug, Clone)]
pub struct ConvertedAsset {
    /// File written beneath the output directory.
    pub path: PathBuf,
    /// Detected derivative media type.
    pub media_type: String,
    /// Page, cells, figure, or other original location.
    pub selection: Selection,
    /// Pixel dimensions; present exactly when the asset is a raster image.
    pub pixel_size: Option<PixelSize>,
    /// Caption and where its text came from; `None` when the source gives the asset none.
    pub caption: Option<AssetCaption>,
}

/// Pixel dimensions of a raster image asset.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PixelSize {
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
}

/// A caption with its origin, so generated text is never shown as the document's own.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AssetCaption {
    /// Caption text.
    pub text: String,
    /// Where the text came from.
    pub origin: CaptionOrigin,
}

/// Where a caption's text came from, as far as a converter can know.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CaptionOrigin {
    /// The document's own caption or alternative text.
    Source,
    /// Text the conversion process generated.
    Process,
}
```

and append at the end of the file, after `impl Default for ConversionSettings`:

```rust

impl CaptionOrigin {
    /// The word a read response uses for this origin.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Source => "source",
            Self::Process => "process",
        }
    }
}
```

- [ ] **Step 4: Update the ingest brief.** In `crates/ingest/AGENTS.md` insert this line directly
after the line that starts `- **Docling through its real API.**`:

```markdown
- **An asset says how big it is and whose words its caption is.** For a picture, set `ConvertedAsset.pixel_size` from `PictureImage.width` and `height`, `media_type` from `PictureImage.mimetype`, and `caption` from `Node::Picture.caption` with `CaptionOrigin::Source`. Use `CaptionOrigin::Process` only for text the conversion generated; never label generated text as the source's.
```

- [ ] **Step 5: Format and run.**

```powershell
rustfmt --edition 2024 crates/core/src/conversion.rs crates/core/tests/ports.rs
cargo test --locked -p okf-jawn-core --test ports
bun test ./tests/foundation/ports.test.mjs
```

Expected: `test result: ok. 20 passed; 0 failed`; `8 pass`, `0 fail`. Filtered Clippy check: no
`conversion.rs` line.

- [ ] **Step 6: Commit.**

```powershell
git add crates/core/src/conversion.rs crates/core/tests/ports.rs crates/ingest/AGENTS.md
git commit -m @'
fix(core): give converted assets a pixel size and a caption origin.

Why: the wire MediaReference requires width, height and caption_origin, but
ConvertedAsset had only an optional caption string, so a read would have to
decode each image again and guess whether a caption is the document's own
(SPEC 5: retain images/captions and source locators; origin labels must be
truthful).
What changed: ConvertedAsset.pixel_size is Option<PixelSize> and caption is
Option<AssetCaption { text, origin }> with CaptionOrigin::{Source, Process};
CaptionOrigin::as_str gives the wire words. The ingest brief names the Docling
fields each comes from (PictureImage.width/height/mimetype,
Node::Picture.caption).
Verified: cargo test --locked -p okf-jawn-core --test ports -> 20 passed; bun
test ./tests/foundation/ports.test.mjs -> 8 pass, 0 fail.
Next: F.19 commits the signature guard and runs the package gate.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_01LoZr4ME3crLYjcTNn5GcAC
'@
```

---

### Task F.19: Commit the guard; package gate

**Files:**

- Create (commit): `tests/foundation/ports.test.mjs` (written in F.1, extended in F.15, untracked until now)
- Test: the whole package gate.

**Interfaces:**

- Consumes: everything above.
- Produces: the committed guard; a gate record for the orchestrator.

- [ ] **Step 1: Prove the guard still bites.** In `crates/core/src/search.rs`, temporarily change
the `rebuild` signature's last parameter from `head: Revision` to `head: &'a At`, then run:

```powershell
bun test ./tests/foundation/ports.test.mjs
```

Expected: `7 pass`, `1 fail`, with
``search.rs: SearchIndex::rebuild mentions the `At` selector``. Then restore the file and confirm
it is clean:

```powershell
git checkout -- crates/core/src/search.rs
git status --short
```

Expected: only `?? tests/foundation/ports.test.mjs`.

- [ ] **Step 2: Run the guard green.**

```powershell
bun test ./tests/foundation/ports.test.mjs
```

Expected: `8 pass`, `0 fail`.

- [ ] **Step 3: Compare the import block of `crates/core/tests/ports.rs`** with this reference
(after `rustfmt`; an import that is missing or extra means a step was skipped):

```rust
use std::collections::BTreeMap;
use std::error::Error;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use okf_jawn_contract::access::{AccessRoute, Connector, IssuedConnector, Permission, Principal};
use okf_jawn_contract::common::{PageRequest, TextRange, Warning};
use okf_jawn_contract::error::{ApiError, ErrorCode};
use okf_jawn_contract::events::{Event, EventKind, Receipt};
use okf_jawn_contract::identity::{
    ArtifactId, Digest, ItemId, JobId, MutationId, ProposalId, Revision, TenantId, UploadId,
    WorkspaceId, WorkspacePath,
};
use okf_jawn_contract::import::{Job, JobKind, JobState};
use okf_jawn_contract::item::{Draft, ItemKind, Lifecycle};
use okf_jawn_contract::proposal::{Change, Comment, Proposal, ProposalStatus};
use okf_jawn_contract::read::Selection;
use okf_jawn_contract::review::{Confirmation, Review};
use okf_jawn_contract::search::{GetGraphResponse, LinkDirection};
use okf_jawn_contract::transport::TRANSPORTS;
use okf_jawn_contract::workspace::Workspace;
use okf_jawn_core::confirmations::{ConfirmationConsume, ConfirmationCreate, ConfirmationStore};
use okf_jawn_core::conversion::{
    AssetCaption, CaptionOrigin, ConversionInput, ConversionSettings, ConversionStatus,
    ConvertedAsset, Converter, OcrPolicy, PixelSize,
};
use okf_jawn_core::credentials::{
    ConnectorIssue, CredentialStore, NewConnector, SessionRecord, secret_hash,
};
use okf_jawn_core::drafts::{DraftStore, DraftWrite};
use okf_jawn_core::events::{EventLog, EventQuery, NewEvent};
use okf_jawn_core::jobs::{
    ArtifactKind, ArtifactRecord, ClaimedJob, JobCompletion, JobHandler, JobLease, JobQueue,
    JobSpec, NewArtifact, NewJob, RecordStore,
};
use okf_jawn_core::proposals::{CommentPage, ProposalFilter, ProposalStore};
use okf_jawn_core::sandbox::{SandboxCapabilityStore, SandboxMint, SandboxResolved, token_hash};
use okf_jawn_core::search::{GraphQuery, LinkQuery, SearchIndex, SearchQuery};
use okf_jawn_core::storage::{
    BlameQuery, BlobStore, ByteReader, CandidateChanges, CandidateCheck, CommitChanges, Committed,
    DiffQuery, LocalSource, LogQuery, NewWorkspace, ObjectInfo, Page, Promotion, Provenance,
    StorageScope, TreeEdit, VersionStore, WorkspaceArchive, WorkspaceCatalog, WorkspaceUpdate,
    derive_item_id, derive_proposal_id, workspace_with_permissions,
};
use okf_jawn_core::uploads::{NewUpload, UploadRecord, UploadStore};
use serde_json::json;
use uuid::Uuid;
```

The file's item order must be: these imports; `type TestResult`; `struct OkfConformance`;
`impl CandidateCheck for OkfConformance`; then every `fn` (helpers, `*_calls`, `#[test]`s).

- [ ] **Step 4: Source policy check of the new test file.**

```powershell
Select-String -Path crates/core/tests/ports.rs -Pattern '\.unwrap\(|\.expect\(|expect_err|panic!|unreachable!|todo!|unimplemented!|#\s*!?\[\s*(allow|expect)'
```

Expected: no line.

- [ ] **Step 5: Run the package gate.**

```powershell
cargo fmt --all --check
cargo test --locked -p okf-jawn-core --test ports
bun test ./tests/foundation/ports.test.mjs
```

Expected: `cargo fmt` prints no diff for any file in the "Files allowed" list (a diff in a file
this package does not own is reported to the orchestrator, not fixed here);
`test result: ok. 20 passed; 0 failed`; `8 pass`, `0 fail`.

Then the filtered Clippy check from the package header. Expected: no line. If Clippy's output
contains `error:` lines located in `dispatch.rs`, `mutations.rs`, `context.rs`, `access.rs`,
`tests/dispatch.rs`, `tests/authorization.rs` or `tests/support/**`, record them in the handoff
as package E's; the E0407 error for `find_by_mutation` in `tests/dispatch.rs` is the expected
cross-package break (Deviations 8).

- [ ] **Step 6: Commit.**

```powershell
git add tests/foundation/ports.test.mjs
git commit -m @'
test(foundation): guard port signatures against selectors, principals and wire requests.

Why: three review rounds each found ports that took At, Principal or a wire
*Request type, or kept a lookup by mutation beside an idempotent insert. A
rule that is only written down returns (design 5 F: a compile-time check that
no port signature mentions At or a *Request type).
What changed: tests/foundation/ports.test.mjs reads every file under
crates/core/src that declares a pub trait and fails when a trait fn signature
names At, Principal (outside AccessControl) or a type ending in Request, when
a store keeps a find_*_by_mutation method, when a listed writing method does
not take the write identity, or when a new port file is not covered. It needs
no Rust toolchain and runs with the other foundation tests.
Verified: before the cure the guard reported 21 signature violations and 7
lookups; now bun test ./tests/foundation/ports.test.mjs -> 8 pass, 0 fail;
with rebuild(head: &At) it fails naming SearchIndex::rebuild; cargo test
--locked -p okf-jawn-core --test ports -> 20 passed.
Next: orchestrator verifies package F and, with E merged, runs clippy on
crates/core/tests/ports.rs and confirms tests/dispatch.rs compiles.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_01LoZr4ME3crLYjcTNn5GcAC
'@
```

---

### Package F acceptance

A verifying agent that did not write the package runs, from PowerShell in `D:\okf\cure\ports`:

**1. Scope.**

```powershell
git diff --name-only (git merge-base HEAD integration/foundation-cure) HEAD
```

Expected, exactly these 17 paths:

```text
crates/core/src/application/mod.rs
crates/core/src/confirmations.rs
crates/core/src/conversion.rs
crates/core/src/credentials.rs
crates/core/src/drafts.rs
crates/core/src/events.rs
crates/core/src/jobs.rs
crates/core/src/lib.rs
crates/core/src/proposals.rs
crates/core/src/sandbox.rs
crates/core/src/search.rs
crates/core/src/storage.rs
crates/core/src/uploads.rs
crates/core/tests/ports.rs
crates/ingest/AGENTS.md
crates/storage/AGENTS.md
tests/foundation/ports.test.mjs
```

(`readiness.rs` is unchanged and no new module file was needed.)

**2. Tests.**

| Command | Expected |
| --- | --- |
| `cargo test --locked -p okf-jawn-core --test ports` | `test result: ok. 20 passed; 0 failed` |
| `bun test ./tests/foundation/ports.test.mjs` | `8 pass`, `0 fail` |
| `cargo fmt --all --check` | no diff printed for any of the 17 paths |

**3. Lints in this package's files.** Run the filtered Clippy check from the package header.
Expected: no line. Errors Clippy prints for files owned by package E are not this package's.

**4. The guard guards.** In `crates/core/src/storage.rs` change `fn head<'a>(&'a self, scope: &'a StorageScope)`
to `fn head<'a>(&'a self, scope: &'a StorageScope, at: &'a At)`; run
`bun test ./tests/foundation/ports.test.mjs`; expected `1 fail` naming
`storage.rs: VersionStore::head`. Run `git checkout -- crates/core/src/storage.rs`.

**5. The named proof.** Read `a_commit_is_built_from_a_claimed_job_alone` in
`crates/core/tests/ports.rs`: `commit_for` takes only `&ClaimedJob` and a list of edits and
returns `CommitChanges`; the test asserts the mutation id, base revision and author all came
from the claim.

**5a. The added ports.** In `crates/core/src/jobs.rs`, `cancel_job`, `retry_job` and
`record_artifact` each take `mutation_id: MutationId`, and `get_artifact` takes a `StorageScope`.
Remove the `mutation_id: MutationId,` line from `retry_job`, run the guard, and expect `1 fail`
with `jobs.rs: RecordStore::retry_job does not take MutationId`; then run
`git checkout -- crates/core/src/jobs.rs`. Read
`an_artifact_record_yields_the_wire_download_for_its_transport`: the expected path is built from
the contract's own `download_artifact` template, not from a copied string.

**6. No fakes.**

```powershell
Select-String -Path crates/core/src/*.rs,crates/core/src/application/*.rs -Pattern 'todo!|unimplemented!|unreachable!|#\s*!?\[\s*(allow|expect)'
git grep -n "impl .* for " -- crates/core/src/storage.rs crates/core/src/jobs.rs crates/core/src/conversion.rs
```

Expected: the first prints nothing; the second prints only `impl From<PageRequest> for Page`
and `impl Default for ConversionSettings` — no port trait has an implementation in `crates/core/src`.

**7. After integration (orchestrator, with package E merged).**

```powershell
cargo clippy --locked -p okf-jawn-contract -p okf-jawn-core --all-targets -- -D warnings
cargo test --locked -p okf-jawn-contract -p okf-jawn-core
```

Expected: both exit 0. This is the first run in which Clippy reaches `crates/core/tests/ports.rs`.

### Deviations

1. **No `impl From<proposal::Change> for TreeEdit`; `TreeEdit::from_change(change, new_item_id, new_item_kind)` instead.**
   Evidence: `crates/contract/src/proposal.rs:11-20` — `Change::Create { path, type_name, body, properties }`
   carries no item identity, no `ItemKind` and no title, while `CreateItemRequest`
   (`item.rs:145-164`) carries `kind` and `title`, and a created item's identity must be
   allocated by the caller (source cards in one commit refer to each other through
   `SourceAppearance.parent_item_id`; `crates/storage/Cargo.toml` has no `uuid` dependency with
   which to allocate). A total `From` would have to invent all three. **Contract change that
   would allow the plain `From`:** add `kind: ItemKind` (and optionally `title: String`) to
   `proposal::Change::Create`; the identity would still come from `derive_item_id`.
2. **`find_mutation` is not fully removed: it is `VersionStore::find_commit(scope, mutation_id, since)`.**
   The caller that needs it is `commit_items`: after package C its request has no
   `expected_head`, so a resumed attempt reads a head that already includes its own first
   commit; `commit`'s replay search over `expected_head..head` would miss it and a second
   commit would be written, and the draft-conflict check would fire on the attempt's own change.
   `VersionTarget` is removed; candidates are idempotent through `create_candidate` itself.
3. **`VersionStore::restore` is removed rather than given a message.** Restore is the
   `TreeEdit::RestorePaths` / `RestoreWorkspace` edits inside an ordinary `CommitChanges`, which
   already carries `mutation_id` and `message` and runs the candidate check. The brief asked that
   restore "keeps the request's message and takes mutation_id"; both hold.
4. **`VersionStore::commit` and `promote_candidate` return `Committed { revision, replayed, warnings }`, not `MutationResult`.**
   `MutationResult` (`common.rs:79-86`) contains `receipt_id`, which only `RecordStore` can
   produce. The application inserts the receipt and builds `MutationResult`.
5. **Port changes beyond the thirteen listed items, all inside the allowed files, each because the old shape could not serve a SPEC sentence:**
   - `VersionStore::resolve(At)` → `head()` (the guard forbids `At`; a pinned revision needs no resolving).
   - `VersionStore::list` returns `FolderListing` with child folders and takes a `Page`
     (`ListItemsResponse.folders`, `item.rs:124`, could not be filled).
   - `VersionStore::rules`, `types`, `correction`: typed reads for the typed writes `SetRules`,
     `SetType`, `CorrectDigest`. Core has no YAML library, so it cannot parse
     `.okf/rules.yaml` from `read_file` bytes; storage has `yaml_serde`.
   - `create_candidate` takes `CandidateChanges`; `promote_candidate` takes `Promotion`
     (approver and message).
   - `Provenance.route` is `AccessRoute`, not `String`; `Provenance` is serializable.
   - `derive_item_id`, `derive_proposal_id` (pure helpers in `storage.rs`).
   - `RecordStore::list_reviews(scope, item)` takes no revision at all, and returns `Vec<Review>`.
   - `SandboxMint` / `SandboxResolved` gain `media_type`; `mint` returns `()` and loses
     `Option<MutationId>`.
   - `ConversionInput.file_name`; a document-level failure is a `Conversion` with a status, not an `Err`.
   - `UploadStore` works on `NewUpload` / `UploadRecord` instead of the wire `Upload`.
   - `EventLog::append` takes `Option<MutationId>` and `NewEvent`.
   - `DraftWrite.content_digest`; `DraftStore` uniqueness is `(mutation_id, item)`.
   - `CredentialStore::list_connectors(include_revoked)`; `lookup_connector` takes the hash.
   - The guard also rejects `find_*_by_mutation` methods (item 5 made testable).
6. **Shared test helper not used.** `tests/support/check.rs` is produced by package E in this
   wave. `ports.rs` declares `TestResult` locally and needs neither `err_of` nor `some`. The
   orchestrator may swap in the shared module after integration.
7. **The Clippy gate cannot be green in this worktree, and Clippy does not reach `tests/ports.rs` here.**
   The library target still has package E's lint errors (CI run 37352882339: `access.rs`,
   `dispatch.rs`, `mutations.rs`), so the dependent test target is never linted. This package
   proves only that no diagnostic is located in its own source files. `ports.rs` is linted for
   the first time at integration (acceptance step 7); it was written to the lint policy
   (no `unwrap`/`expect`/`panic!`/indexing; types before `impl` before functions; no
   `format!`-collect; `saturating`/no arithmetic).
8. **Cross-package compile break on this branch.** `crates/core/tests/dispatch.rs:654-730`
   (package E's file) implements `ConfirmationStore::find_by_mutation` (`:684-691`), removed in
   F.10, so the `dispatch` test target stops compiling here. Package E's plan deletes
   `FixtureConfirmationStore` with its test, which resolves it on merge; if E keeps the fixture,
   the orchestrator deletes that one method at integration. The same file uses nothing else
   this package changed (`StorageScope` is untouched).
9. **Manifest and record follow-ups for the integration owner (not done here; outside allowed files):**
   - `crates/storage/Cargo.toml` `runtime` feature has no `uuid` and no random source. Storage
     must allocate `MutationId`, `JobId`, `UploadId`, `ConfirmationId`, `ConnectorId`,
     `WorkspaceId` and generate connector secrets ("at least 32 bytes from the operating
     system's random source").
   - Core generates sandbox tokens and has no random source other than `uuid`'s `v4`.
   - `verification.json` gate meanings for `storage-git-cas-sqlite` and
     `mutation-crash-reconcile` still say `find_mutation`; the cured behaviour is "retry calls
     `commit` with the same `MutationId` and gets the first revision, `replayed: true`".
10. **Gaps still unplanned** (Tasks F.15–F.18 closed job retry/cancel identity, comment
    listing, artifact records and asset size/caption origin; each item below needs a contract or
    product decision, or is lane work by ruling):
    - No port lets a handler assemble an export or backup or apply a restore: nothing enumerates
      the tracked paths of a revision (`read_file` needs a known path), bundles Git history, or
      dumps and reloads `RecordStore` rows. The archive format and the three handlers are lane
      work by ruling; the ports they need are designed with them (SPEC §12 "Restore must be
      exercised").
    - The wire cannot return a discussion: `Proposal` has no comments field and the operation
      table has no comment-listing operation, so `ProposalStore::list_comments` has no caller
      until the contract gains one or the other.
    - `GetSourcesResponse.sources` (where an item's citations persist) is a product and format
      decision; it stays in "For the storage-lane plan", item 13.
    - `MediaReference` requires `caption: String`, `width: u32` and `height: u32`
      (`read.rs:133-147`), while a converted asset may have no caption and, when it is not a
      raster image, no pixel size. Whether those wire fields become optional, or such assets are
      never returned as media, is a contract decision.
    - `CaptionOrigin::Process` cannot occur with today's `ConversionSettings`: Docling's only
      generated label comes from picture classification, which the settings do not expose.
11. **The guard reads two files package E owns** (`access.rs`, `mutations.rs`) without changing
    them. Neither violates a rule before or after E.
12. **Task F.1 ends without a commit.** The guard is red until F.12 and a red test is never
    committed; it is committed in F.19.
13. **`readiness.rs` is unchanged.** `ReadinessProbe::probe(&self) -> PortFuture<'_, ReadinessResponse>`
    already takes no selector, principal or request and has no lint finding.
14. **`WorkspaceCatalog::list` omits archived workspaces and `open` returns them.** The wire has
    no archived flag on `Workspace` and no "include archived" on `ListWorkspacesRequest`; this is
    the reading that keeps retained content reachable. If the owner wants archived workspaces
    listed, the contract needs a flag.

15. **Brief update for the orchestrator: `crates/core/AGENTS.md` (not this package's file).**
    The production `CandidateCheck` is core-lane work: implement `storage::CandidateCheck` over
    `okf_core::Bundle::load` and `okf_validator::validate_bundle`, pass it to every
    `VersionStore::commit` and `create_candidate`, and decide the rule for errors already in the
    tree (storage-lane list, item 5). `crates/core/tests/ports.rs` holds its shape as the test
    type `OkfConformance`. The core brief should say so.
16. **Consequences of Tasks F.15–F.18 beyond their four headlines.** `JobCompletion.artifact`
    is `Option<ArtifactId>` (was the wire `DownloadArtifact`), with `artifact_download_path` and
    `ArtifactRecord::download` defining the wire value once; `ConvertedAsset.caption` is
    `Option<AssetCaption>` (was `Option<String>`); `ProposalStore::list_comments` returns a core
    `CommentPage` because the contract has no response type for it; and the guard gained a
    fixed list of writing methods that must take the write identity.

### For the storage-lane plan

okf-core / okf-validator 0.2.7 model mismatches with the contract. Each needs a contract or
product decision before, or while, storage implements `VersionStore`; none is planned as a task
here. Library references are to `~/.cargo/registry/src/index.crates.io-*/okf-core-0.2.7/src/`;
contract line numbers are those of `b205c4a`, before package C.

1. **Lifecycle words.** Wire `Lifecycle { Active, Deprecated, Archived }` (`item.rs:21-28`) vs
   OKF `Status { Draft, Stable (default when absent), Deprecated, Other(String) }`
   (`trust.rs:223`, `STATUS_VALUES = ["draft", "stable", "deprecated"]`). OKF has no `archived`;
   the wire has no `draft` and no place for `Other`. What is written to `status:` for `Archived`,
   and what an imported `status: draft` or unknown value shows as, is undecided. (Design §10
   already asks whether exports use the product's words or OKF's.)
2. **A stable `ItemId` has no home in OKF.** A concept's identity is `ConceptId` = "a concept's
   path within the bundle, minus `.md`" (`lib.rs`), and `move_concept` changes it. Nothing in
   okf-core persists a UUID. Candidates: a frontmatter extension key (portable, survives export,
   visible to users), or a path↔id map in SQLite (invisible, lost on plain-folder export and on
   restore from Git alone). SPEC §3 "A rename must not make historical references point to
   today's content" and §12 "The final export should work without the polished interface" both
   bear on it.
3. **`ItemKind` (Note / Source / View) has no OKF counterpart.** Where the rendering role is
   persisted (a reserved `type` value would collide with "User type names are not a closed
   enum"; an extension key is the alternative). Also the contract gap in Deviations 1.
4. **Frontmatter order and value model.** Wire properties are
   `BTreeMap<String, serde_json::Value>` (sorted). OKF `Frontmatter` wraps an order-preserving
   `Mapping { entries: Vec<(Value, Value)> }` whose `Value` is
   `Null | Bool | Int(i64) | Float(f64) | String | Sequence | Mapping` (`yaml/mod.rs:64,170`).
   An edit round trip through the wire sorts keys (noisy diffs and blame), mapping keys need not
   be strings, and JSON numbers outside `i64`/`f64` have no image. `Document::serialize` also
   re-emits flow collections in block style (`document.rs:108`), so an untouched file can change
   bytes when rewritten.
5. **The YAML parser is a subset.** `Value::parse` "Returns `YamlError` for any input outside the
   supported subset (anchors, tags, multiple documents…)". Such a file lands in
   `Bundle::parse_errors`, and `validate_bundle` reports it as an Error ("unparseable concept
   document"). If the candidate check rejects on any Error, one imported file with an anchor
   blocks every later commit in the workspace. The check needs a rule for pre-existing errors.
6. **Required fields.** Wire `ItemSummary.title` and `description` are required strings; OKF
   requires only `type` (`REQUIRED_FRONTMATTER_KEYS = ["type"]`). `Concept::display_title` falls
   back to the id's last segment; there is no fallback for a description.
7. **Link positions.** Wire `Link.line: u32` is required (`search.rs:78`). OKF `Link { text,
   target, kind }` and `ResolvedLink { target, exists, text, raw }` carry no position, and the
   line-aware scanner (`code_free_lines`) is private. Storage must rescan bodies itself or the
   contract must make `line` optional.
8. **Line numbers for blame and line selections.** git2 blames file lines, frontmatter included
   (`BlameOptions::min_line/max_line`); the wire `BlameRequest.lines` and `Selection::Lines`
   read naturally as lines of the Markdown body. `BlameQuery.lines` is documented here as body
   lines, so storage translates by the frontmatter's length at that revision. Confirm.
9. **Every `.md` file is a concept.** `collect_markdown` recurses into every directory, dot
   directories included, and takes every `*.md`; `index.md` and `log.md` are reserved "at any
   level" (`RESERVED_FILENAMES`). So: correction sidecars and anything under `.okf/` must not end
   in `.md` or they become concepts and are validated; a user cannot have a note named
   `index.md` or `log.md`; non-Markdown files are invisible to the bundle.
10. **Concept paths end in `.md`.** A source card for `report.pdf` needs a concept path; whether
    it is `report.pdf.md`, `report.md` (collides with a sibling `report.docx`), or something
    else interacts with naming rules (`ExtensionPolicy`) and with `WorkspacePath`, which allows
    characters `is_portable_segment` warns about and forbids `:`.
11. **okf-core refactors write to disk and stamp the wall clock.** `move_concept`,
    `remove_concept`, `regenerate_indexes`, `init_bundle` take a directory and write files;
    `update_log` appends to `log.md` with `Date::today_utc()`, and scaffolding stamps
    `current_iso_timestamp()`. A re-run after a crash produces different bytes, so idempotency
    must rest on the commit trailer, never on tree equality. `MoveOptions.author` is an
    `Option<String>`; how `Provenance` maps to it and to the Git signature (name, email) is
    undecided, as is who writes change-log entries for edits that are not moves (SPEC §3
    "Maintain folder index.md documents and a change log").
12. **Trust and review words.** OKF `verified: { by: human:walter, at: … }` yields
    `TrustTier::HumanReviewed`. SPEC §8: "Never infer review from … the string human: in a
    file"; the wire has `ReviewCoverage::Imported`. Whether an in-app review is written into
    frontmatter as `verified` (and so travels with an export), and how an imported `verified` is
    shown, is undecided. (Design §10 lists the export half.)
13. **Citations.** OKF `sources[]` entries (`Source` with `resource`, optional `id`) name a
    resource; a wire `SourceReference` needs workspace, item id, path, revision, optional digest
    and a selection. Where revision and selection live in frontmatter, and how
    `GetSourcesResponse.sources` is derived, is undecided.
14. **Workspace name and description.** The root `index.md` may carry frontmatter only for
    `okf_version` ("the only place frontmatter is permitted in an `index.md`"). Whether
    name/description are catalog rows only (then `update_workspace` produces no new revision,
    yet its wire result is a `MutationResult` with a revision) or versioned somewhere in the
    bundle is undecided.
15. **Conversion state and corrections have no OKF or wire field.** `Extraction` (pending,
    unsupported, failed, partial) and the correction text recorded by `CorrectDigest` must be
    persisted in the tree under keys or files storage chooses, and the wire `ItemSummary` /
    `ItemDocument` cannot show either: `ItemDocument` has one `body`, and no field says
    "extraction unsupported" (SPEC §5 "the item shows that state accurately").
16. **Type definitions and rules are not OKF.** `TypeDefinition` (JSON Schemas) and
    `.okf/rules.yaml` live beside the bundle; okf-validator ignores them, and
    `check_path_fields` resolves path-valued frontmatter against the directory, so their layout
    under `.okf/` should avoid looking like bundle content.
