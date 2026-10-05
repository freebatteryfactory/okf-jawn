## Package C: Contract

- **Branch:** `cure/contract`, created from `integration/foundation-cure` at `678f919`.
- **Worktree:** `D:\okf\cure\contract` (builds into its own `target\`).
- **Wave:** 1, in parallel with A (scripts, `tests/foundation/{policy,vendor}.test.mjs`, CI,
  manifests, docs) and B (`qualification/**`).
- **Files allowed (exact):**
  - Owned: `crates/contract/src/**`, `crates/contract/tests/**`.
  - Minimum edits to keep the workspace compiling, and nothing else in these files:
    `crates/core/src/ports.rs`, `crates/core/src/dispatch.rs`,
    `crates/core/tests/dispatch.rs` (one token), `crates/core/tests/support/counting.rs`
    (macro pattern), `crates/server/src/lib.rs`, `crates/cli/src/lib.rs`, `xtask/src/api.rs`,
    `tests/support/application.rs`.
  - Final commit only: regenerated `api/**`, `generated/cli/**`, `ui/src/api/generated/**`
    (written by `bun scripts/dev.mjs gen`, never by hand), plus
    `ui/src/features/history/Changes.tsx` and `ui/src/mcp-apps/main.tsx`.
- **Must not touch:** `tests/foundation/**`, `scripts/**`, `qualification/**`, any `Cargo.toml`,
  `Cargo.lock`, `package.json`, `bun.lock`, `SPEC.md`, `README.md`, `AGENTS.md`,
  `REPOSITORY-TREE.txt`, `verification.json`, `vendors.json`, `.github/**`. Do not run rustfmt
  on a file outside `crates/contract/` (nine of them are unformatted at base and belong to
  other packages).
- **SPEC/AGENTS sentences served:**
  - SPEC §14: "deny all/pedantic plus selected panic/unchecked-operation lints. No
    allow/expect/cfg_attr suppression" (boxing `ApiError.detail` cures `result_large_err`).
  - SPEC §14: "Rust identities and descriptions are authoritative for wire semantics" and
    "test omitted/null, tagging, naming and resolved revisions semantically across boundaries".
  - SPEC §9: "Generate MCP schemas and annotations from the declared operation semantics" and
    "Model tools cover listing, search, reading, …".
  - SPEC §8: "Typed wire errors expose AlreadyIssued, InProgress and draft/idempotency Conflict
    details the UI can act on"; "`read_item`, search, export, MCP tools and agent routes never
    see drafts".
  - SPEC §10: "Bindings retain document, resolved revision, digest, selection, units and
    transformations."
  - SPEC §3: "Canonical operation identifiers use verb_noun … UI terms are projections".
  - Design §3 owner decisions: Snapshot conflict, Agent discovery, Saved Views, Unbuilt
    operations (501), Tenant-level reads.
  - AGENTS.md generator-input rule: "A lane may change its own authored generator inputs, run
    `bun scripts/dev.mjs gen`, and commit the generated outputs with the lane change."

All cargo commands run from PowerShell in `D:\okf\cure\contract`. Every commit message uses the
shared format. Tests follow the shared rules (return `Result`, `?`, `assert!`/`assert_eq!`; no
`unwrap`, `expect`, `panic!`, indexing, `#[allow]`, `#[expect]`); `tests/support/check.rs` does
not exist yet in wave 1 (see Deviations), so these tests use only `std`.

---

### Task C.0: Set up and record the baseline (no commit)

**Files:** Create (untracked, gitignored) `.artifacts\cure-contract\*`.

**Interfaces:** Consumes nothing. Produces `clippy-before.txt`, `fmt-before.txt`,
`test-before.txt` used by Task C.11 and by the verifier.

- [ ] Create the worktree and install: 
  ```powershell
  git -C C:\Users\eayou\code_dir\okf-jawn worktree add -b cure/contract D:\okf\cure\contract 678f919
  Set-Location D:\okf\cure\contract
  bun scripts/dev.mjs bootstrap
  git status --porcelain
  ```
  Expected: bootstrap ends with `Locked installation, real generation, TypeScript type check,
  and UI consumer build completed.` and `git status --porcelain` prints nothing.
- [ ] Record the baselines:
  ```powershell
  New-Item -ItemType Directory -Force .artifacts\cure-contract | Out-Null
  cargo fmt --all --check 2>&1 | Select-String '^Diff in' | ForEach-Object { ($_.Line -replace ':\d+:$','') } | Sort-Object -Unique | Set-Content .artifacts\cure-contract\fmt-before.txt
  cargo clippy --locked -p okf-jawn-contract -p okf-jawn-core -p okf-jawn-server -p okf-jawn-cli -p okf-jawn-mcp -p xtask --all-targets --keep-going -- -D warnings 2>&1 | Tee-Object .artifacts\cure-contract\clippy-before.txt | Out-Null
  cargo test --locked -p okf-jawn-contract -p okf-jawn-core -p okf-jawn-server -p xtask 2>&1 | Tee-Object .artifacts\cure-contract\test-before.txt | Select-String '^test result|FAILED|error'
  ```
  Expected:
  - `fmt-before.txt` names exactly 11 files: `crates\contract\src\scope.rs`,
    `crates\contract\tests\scope.rs`, `crates\core\src\dispatch.rs`, `crates\core\src\drafts.rs`,
    `crates\core\src\mutations.rs`, `crates\core\tests\dispatch.rs`,
    `crates\core\tests\support\counting.rs`, `crates\core\tests\support\mod.rs`,
    `crates\server\tests\bindings.rs`, `qualification\docling\src\main.rs`, `xtask\src\api.rs`.
  - `clippy-before.txt` contains `could not compile `okf-jawn-core` (lib) due to 28 previous
    errors` (CI run 37352882339 on `b205c4a`): 15 `result_large_err`, 3
    `arbitrary_source_item_ordering`, 5 `elidable_lifetime_names`, 3 `doc_markdown`, 1
    `needless_pass_by_value`, 1 `format_collect`. `xtask` and the contract test targets were
    never linted by CI; whatever this file shows for them is the baseline.
  - every `test result:` line says `ok`. If one fails at base, stop and report; it is not C's.
- [ ] Summarise the clippy baseline by lint and crate for later comparison:
  ```powershell
  function Summarize($file) { Select-String -Path $file -Pattern 'index\.html#(\w+)' | ForEach-Object { $_.Matches[0].Groups[1].Value } | Group-Object | Sort-Object Name | ForEach-Object { '{0,3} {1}' -f $_.Count, $_.Name } }
  Summarize .artifacts\cure-contract\clippy-before.txt
  ```

---

### Task C.1: Format the contract crate

**Files:** Modify `crates/contract/src/scope.rs:8-45` (import block only),
`crates/contract/tests/scope.rs:234,324,340` (three wrapped expressions).

**Interfaces:** No signature changes.

- [ ] Run (expected failure, 5 hunks in the two files above):
  ```powershell
  cargo fmt -p okf-jawn-contract --check
  ```
- [ ] Format: `cargo fmt -p okf-jawn-contract`
- [ ] Run: `cargo fmt -p okf-jawn-contract --check` → no output, exit 0.
  `git diff --stat` → only the two files above.
- [ ] Run: `cargo test --locked -p okf-jawn-contract` → every `test result:` line `ok`.
- [ ] Commit. For every commit in this package: `git add` the listed paths, save the message
  text exactly as shown to `.artifacts\cure-contract\msg.txt` (UTF-8, no BOM), then
  `git commit -F .artifacts\cure-contract\msg.txt`.
  ```powershell
  git add crates/contract/src/scope.rs crates/contract/tests/scope.rs
  ```
  ```text
  chore(contract): format the contract crate with rustfmt.

  Why: cargo fmt --check fails in 11 files on b205c4a; two are in crates/contract (SPEC 14 source conventions; AGENTS.md "Do not suppress lints").
  What changed: import wrapping in src/scope.rs and three wrapped expressions in tests/scope.rs; no behaviour.
  Verified: cargo fmt -p okf-jawn-contract --check exits 0; cargo test --locked -p okf-jawn-contract passes.
  Next: box ApiError.detail.

  Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>
  Claude-Session: https://claude.ai/code/session_01LoZr4ME3crLYjcTNn5GcAC
  ```

---

### Task C.2: Box `ApiError.detail`; add `with_field`

**Files:**
- Create: `crates/contract/tests/error.rs`
- Modify: `crates/contract/src/error.rs:1-4` (header), `:87-89` (field), `:111-116` (`with_detail`)
- Modify (minimum edit): `crates/core/tests/dispatch.rs:454`

**Interfaces:**
- Produces:
  ```rust
  pub struct ApiError { /* … */ pub detail: Option<Box<ErrorDetail>> }
  impl ApiError {
      pub fn with_detail(self, detail: ErrorDetail) -> Self;   // boxes internally
      pub fn with_field(self, field: impl Into<String>) -> Self;
  }
  ```
- Wire shape is unchanged (`Box<T>` serializes and schematizes as `T`; schemars 1.2.2
  `json_schema_impls/wrapper.rs:12`: `wrapper_impl!(<T: ?Sized> JsonSchema for Box<T>);`).

- [ ] Write the failing test. Create `crates/contract/tests/error.rs`:
  ```rust
  //! `ApiError` stays small enough to return by value and keeps typed context on the wire.

  use std::error::Error;

  use okf_jawn_contract::error::{ApiError, ErrorCode, ErrorDetail};
  use serde_json::json;

  // Clippy's `result_large_err` fires at 128 bytes; every `Result<_, ApiError>` relies on this.
  const _: () = assert!(std::mem::size_of::<ApiError>() < 128);

  #[test]
  fn typed_detail_is_unchanged_on_the_wire() -> Result<(), Box<dyn Error>> {
      let connector_id = serde_json::from_value(json!("44444444-4444-4444-8444-444444444444"))?;
      let error = ApiError::new(ErrorCode::AlreadyIssued, "Connector already issued")
          .with_detail(ErrorDetail::AlreadyIssued { connector_id });
      assert_eq!(
          serde_json::to_value(&error)?,
          json!({
              "code": "already_issued",
              "message": "Connector already issued",
              "detail": {
                  "kind": "already_issued",
                  "connector_id": "44444444-4444-4444-8444-444444444444"
              }
          })
      );
      Ok(())
  }
  ```
  (`assert!` in a `const` item is allowed under the repo lints: `clippy::panic` only matches the
  `panic!` macro and skips const contexts; the crate already uses a `const _: () = { … };`
  check at `crates/contract/src/metadata.rs:25-28`.)
- [ ] Run: `cargo test --locked -p okf-jawn-contract --test error`
  Expected failure, at compile time, without building any other crate:
  `error[E0080]: evaluation of constant value failed` … `assertion failed:
  std::mem::size_of::<ApiError>() < 128`. This is the size proof: the same command passing
  after the change shows the drop below Clippy's threshold.
- [ ] Implement in `crates/contract/src/error.rs`:
  - `:3-4` → 
    ```rust
    //! `detail` carries the typed outcome a client can act on; adapters map `conflict`,
    //! `already_issued` and `in_progress` to HTTP 409 with this body. `detail` is boxed so that
    //! `Result<_, ApiError>` stays small.
    ```
  - `:89` `pub detail: Option<ErrorDetail>,` → `pub detail: Option<Box<ErrorDetail>>,`
  - `:114` `self.detail = Some(detail);` → `self.detail = Some(Box::new(detail));`
- [ ] Implement in `crates/core/tests/dispatch.rs:454`: `} = detail` → `} = *detail`
  (the `expect` on the line above now yields `Box<ErrorDetail>`; package E rewrites this test).
- [ ] Run: `cargo test --locked -p okf-jawn-contract --test error` → `test result: ok. 1 passed`.
- [ ] Add the second failing test to `crates/contract/tests/error.rs`. In
  `typed_detail_is_unchanged_on_the_wire`, insert before the `assert_eq!`:
  ```rust
      let detail: &ErrorDetail = error.detail.as_deref().ok_or("detail must be present")?;
      assert_eq!(detail, &ErrorDetail::AlreadyIssued { connector_id });
  ```
  and append:
  ```rust
  #[test]
  fn with_field_names_the_input_to_correct() -> Result<(), Box<dyn Error>> {
      let error = ApiError::new(ErrorCode::InvalidInput, "path must be relative").with_field("/path");
      assert_eq!(error.field.as_deref(), Some("/path"));
      assert_eq!(serde_json::to_value(&error)?.get("field"), Some(&json!("/path")));
      Ok(())
  }
  ```
- [ ] Run: `cargo test --locked -p okf-jawn-contract --test error`
  Expected failure: `error[E0599]: no method named `with_field` found for struct `ApiError``.
- [ ] Implement: append to `impl ApiError` in `crates/contract/src/error.rs` (after `with_detail`):
  ```rust

      /// Name the input field that needs correction.
      #[must_use]
      pub fn with_field(mut self, field: impl Into<String>) -> Self {
          self.field = Some(field.into());
          self
      }
  ```
- [ ] Run:
  ```powershell
  cargo fmt -p okf-jawn-contract
  cargo test --locked -p okf-jawn-contract -p okf-jawn-core -p okf-jawn-server -p xtask
  ```
  Expected: every `test result:` line `ok`; `error` target shows `2 passed`.
- [ ] Commit `git add crates/contract/src/error.rs crates/contract/tests/error.rs crates/core/tests/dispatch.rs`:
  ```text
  fix(contract): box ApiError.detail and add with_field.

  Why: ApiError was over 128 bytes, so every Result<_, ApiError> tripped clippy::result_large_err (15 errors in okf-jawn-core, CI run 37352882339); dispatch also needs to name the offending input field.
  What changed: ApiError.detail is Option<Box<ErrorDetail>>; with_detail boxes; new ApiError::with_field. Wire shape unchanged. crates/core/tests/dispatch.rs:454 dereferences the box.
  Verified: cargo test --locked -p okf-jawn-contract --test error fails with E0080 before and passes after (2 passed); cargo test --locked -p okf-jawn-contract -p okf-jawn-core -p okf-jawn-server -p xtask passes.
  Next: ErrorCode::NotImplemented.

  Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>
  Claude-Session: https://claude.ai/code/session_01LoZr4ME3crLYjcTNn5GcAC
  ```

---

### Task C.3: `ErrorCode::NotImplemented`

**Files:**
- Modify: `crates/contract/src/error.rs:35-36` (new last variant), `crates/contract/tests/error.rs`
- Modify (minimum edit): `crates/server/src/lib.rs:74`

**Interfaces:** Produces `ErrorCode::NotImplemented`, wire `"not_implemented"`, HTTP 501.

- [ ] Write the failing test; append to `crates/contract/tests/error.rs`:
  ```rust
  #[test]
  fn not_implemented_is_a_wire_code_of_its_own() -> Result<(), Box<dyn Error>> {
      assert_eq!(serde_json::to_value(ErrorCode::NotImplemented)?, json!("not_implemented"));
      let decoded: ErrorCode = serde_json::from_value(json!("not_implemented"))?;
      assert_eq!(decoded, ErrorCode::NotImplemented);
      Ok(())
  }
  ```
- [ ] Run: `cargo test --locked -p okf-jawn-contract --test error`
  Expected failure: `error[E0599]: no variant or associated item named `NotImplemented` found
  for enum `ErrorCode``.
- [ ] Implement in `crates/contract/src/error.rs`, after the `Internal,` variant (`:36`):
  ```rust
      /// The operation is declared but this build does not implement it; never a success.
      NotImplemented,
  ```
- [ ] Implement in `crates/server/src/lib.rs`, after line 74
  (`ErrorCode::Internal => StatusCode::INTERNAL_SERVER_ERROR,`):
  ```rust
          ErrorCode::NotImplemented => StatusCode::NOT_IMPLEMENTED,
  ```
- [ ] Run:
  ```powershell
  cargo fmt -p okf-jawn-contract
  cargo test --locked -p okf-jawn-contract -p okf-jawn-core -p okf-jawn-server -p xtask
  ```
  Expected: all `ok`; `error` target `3 passed`.
- [ ] Commit `git add crates/contract/src/error.rs crates/contract/tests/error.rs crates/server/src/lib.rs`:
  ```text
  feat(contract): add ErrorCode::NotImplemented, bound to HTTP 501.

  Why: owner decision "Unbuilt operations": the generated skeleton returns a typed not-implemented error, never a success.
  What changed: ErrorCode gains NotImplemented (wire "not_implemented"); the server status match maps it to 501 so the workspace keeps compiling.
  Verified: cargo test --locked -p okf-jawn-contract --test error failed with E0599 before, 3 passed after; cargo test --locked -p okf-jawn-contract -p okf-jawn-core -p okf-jawn-server -p xtask passes.
  Next: 13-field operation table.

  Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>
  Claude-Session: https://claude.ai/code/session_01LoZr4ME3crLYjcTNn5GcAC
  ```

---

### Task C.4: 13-field operation table; delete `labels.rs`; `workspaces` tool

**Files:**
- Create: `crates/contract/tests/operations.rs`
- Modify: `crates/contract/src/operations.rs:1-6` (header), `:13-81` (all 69 rows)
- Modify: `crates/contract/src/metadata.rs:12-30` (macro), `:56-77` (`OperationInfo`)
- Modify: `crates/contract/src/lib.rs:26`, `:49-51`; Delete: `crates/contract/src/labels.rs`
- Modify: `crates/contract/tests/scope.rs:19-20`
- Modify (tuple consumers, same commit): `crates/core/src/ports.rs:11-12`,
  `crates/core/src/dispatch.rs:36-37`, `crates/core/tests/support/counting.rs:13-14`,
  `crates/server/src/lib.rs:34-35`, `xtask/src/api.rs:21-22`, `:25-29`, `:443`,
  `tests/support/application.rs:10-11`, `crates/cli/src/lib.rs:4`, `:33-37`

**Interfaces:**
- Produces the shared 13-field tuple
  ```text
  ($id:ident, $request:ty, $response:ty, $path:literal, $label:literal, $alias:literal,
   $operator:literal, $visibility:literal, $permission:ident, $ui:literal, $status:literal,
   $destructive:literal, $description:literal)
  ```
  and
  ```rust
  pub struct OperationInfo {
      pub id: &'static str, pub path: &'static str, pub label: &'static str,
      pub alias: &'static str, pub operator_alias: &'static str, pub visibility: &'static str,
      pub permission: Permission, pub ui: &'static str, pub success_status: u16,
      pub destructive: bool, pub description: &'static str,
  }
  ```
- `okf_jawn_contract::labels` no longer exists.
- Totals: 69 operations, 12 model tools, 1 app tool.

- [ ] Write the failing test. Create `crates/contract/tests/operations.rs`:
  ```rust
  //! The operation table is the only source of agent exposure, operator vocabulary and hints.

  use std::collections::BTreeSet;
  use std::error::Error;

  use okf_jawn_contract::access::Permission;
  use okf_jawn_contract::metadata::{OperationInfo, operations};

  /// Operations that carry or reveal a draft; agents never see drafts (SPEC section 8).
  const DRAFT_BEARING: &[&str] = &[
      "save_draft",
      "list_drafts",
      "discard_draft",
      "commit_items",
      "get_item",
  ];
  /// Operations that destroy or overwrite user state, in table order.
  const DESTRUCTIVE: &[&str] = &[
      "archive_workspace",
      "restore_workspace",
      "discard_draft",
      "delete_item",
      "restore_items",
      "apply_names",
      "revoke_connector",
  ];
  /// Operator and CLI vocabulary, in table order.
  const OPERATOR_ALIASES: &[(&str, &str)] = &[
      ("export_workspace", "export"),
      ("log_items", "timeline"),
      ("diff_items", "changes"),
      ("commit_items", "snapshot"),
      ("restore_items", "rewind"),
      ("blame_item", "who"),
      ("accept_proposal", "approve"),
      ("decline_proposal", "decline"),
      ("create_review", "verify"),
      ("start_import", "import"),
      ("get_attention", "attention"),
  ];
  /// Model tool names, in table order.
  const MODEL_TOOLS: &[&str] = &[
      "workspaces",
      "ls",
      "show",
      "sources",
      "grep",
      "links",
      "log",
      "diff",
      "blame",
      "propose",
      "present",
      "catalog",
  ];

  fn declared(id: &str) -> Result<OperationInfo, Box<dyn Error>> {
      operations()
          .into_iter()
          .find(|operation| operation.id == id)
          .ok_or_else(|| format!("{id} is not declared").into())
  }

  #[test]
  fn the_surface_is_69_operations_12_model_tools_and_1_app_tool() {
      let table = operations();
      assert_eq!(table.len(), 69);
      let model: Vec<&str> = table
          .iter()
          .filter(|operation| operation.visibility == "model")
          .map(|operation| operation.alias)
          .collect();
      assert_eq!(model, MODEL_TOOLS);
      let app: Vec<&str> = table
          .iter()
          .filter(|operation| operation.visibility == "app")
          .map(|operation| operation.alias)
          .collect();
      assert_eq!(app, ["read_object"]);
  }

  #[test]
  fn only_read_and_propose_operations_are_visible_to_agents() {
      for operation in operations() {
          assert!(
              matches!(operation.visibility, "" | "model" | "app"),
              "{} has unknown visibility {}",
              operation.id,
              operation.visibility
          );
          assert_eq!(
              operation.alias.is_empty(),
              operation.visibility.is_empty(),
              "{} has a tool alias exactly when it is visible to agents",
              operation.id
          );
          if !operation.visibility.is_empty() {
              assert!(
                  matches!(operation.permission, Permission::Read | Permission::Propose),
                  "{} is agent-visible but requires {:?}",
                  operation.id,
                  operation.permission
              );
              assert!(
                  !operation.destructive,
                  "{} is agent-visible and destructive",
                  operation.id
              );
          }
      }
  }

  #[test]
  fn draft_bearing_operations_are_never_agent_tools() -> Result<(), Box<dyn Error>> {
      // `alias` and `visibility` are the agent surface; `operator_alias` is CLI vocabulary.
      for id in DRAFT_BEARING {
          let operation = declared(id)?;
          assert!(
              operation.alias.is_empty(),
              "{id} carries drafts and must not have a tool alias"
          );
          assert!(
              operation.visibility.is_empty(),
              "{id} carries drafts and must not be visible to agents"
          );
      }
      Ok(())
  }

  #[test]
  fn destructive_hints_come_from_the_table() {
      let destructive: Vec<&str> = operations()
          .iter()
          .filter(|operation| operation.destructive)
          .map(|operation| operation.id)
          .collect();
      assert_eq!(destructive, DESTRUCTIVE);
  }

  #[test]
  fn operator_vocabulary_comes_from_the_table_and_names_one_command() {
      let table = operations();
      let aliases: Vec<(&str, &str)> = table
          .iter()
          .filter(|operation| !operation.operator_alias.is_empty())
          .map(|operation| (operation.id, operation.operator_alias))
          .collect();
      assert_eq!(aliases, OPERATOR_ALIASES);
      let mut names = BTreeSet::new();
      for operation in &table {
          let mut own = BTreeSet::from([operation.id]);
          own.extend(
              [operation.alias, operation.operator_alias]
                  .into_iter()
                  .filter(|name| !name.is_empty()),
          );
          for name in own {
              assert!(names.insert(name), "{name} names more than one command");
          }
      }
  }

  #[test]
  fn the_workspaces_tool_tells_the_agent_to_pin_reads() -> Result<(), Box<dyn Error>> {
      let workspaces = declared("list_workspaces")?;
      assert_eq!(workspaces.alias, "workspaces");
      assert_eq!(workspaces.visibility, "model");
      assert_eq!(workspaces.permission, Permission::Read);
      for phrase in [
          "head revision",
          "pin",
          r#"{"kind": "revision", "revision": head}"#,
          "detect change",
      ] {
          assert!(
              workspaces.description.contains(phrase),
              "the description must say: {phrase}"
          );
      }
      Ok(())
  }
  ```
- [ ] Run: `cargo test --locked -p okf-jawn-contract --test operations`
  Expected failure: `error[E0609]: no field `destructive` on type `OperationInfo`` (and the
  same for `operator_alias`).
- [ ] Rewrite the table rows mechanically. Save this as
  `.artifacts\cure-contract\c4-table.mjs` (gitignored; it was run against the `b205c4a` file
  while planning and rewrote 69 rows):
  ```js
  // One-off rewrite of the operation table from 11 to 13 fields. Run once from the worktree root.
  import { readFileSync, writeFileSync } from 'node:fs';

  const file = process.argv[2] ?? 'crates/contract/src/operations.rs';
  const operator = {
    log_items: 'timeline',
    diff_items: 'changes',
    commit_items: 'snapshot',
    restore_items: 'rewind',
    blame_item: 'who',
    accept_proposal: 'approve',
    decline_proposal: 'decline',
    start_import: 'import',
    export_workspace: 'export',
    get_attention: 'attention',
    create_review: 'verify',
  };
  const destructive = new Set([
    'archive_workspace',
    'restore_workspace',
    'discard_draft',
    'delete_item',
    'restore_items',
    'apply_names',
    'revoke_connector',
  ]);
  const workspacesDescription =
    'List the workspaces this connection may read, each with its current head revision. Treat head as a pin: pass it to later reads as at = {\\"kind\\": \\"revision\\", \\"revision\\": head} so every call sees one consistent state, and compare it with a later listing to detect change.';
  const row =
    /^(\s*\()(\w+)(, \$crate::[\w:]+, \$crate::[\w:]+, "[^"]+", "[^"]*", )"([^"]*)", "([^"]*)"(, \w+, "[^"]*", \d+), "(.*)"\),(\r?)$/;
  let rows = 0;
  const lines = readFileSync(file, 'utf8')
    .split('\n')
    .map((line) => {
      const match = row.exec(line);
      if (!match) return line;
      rows += 1;
      const [, open, id, head, alias, visibility, middle, description, end] = match;
      const workspaces = id === 'list_workspaces';
      return `${open}${id}${head}"${workspaces ? 'workspaces' : alias}", "${operator[id] ?? ''}", "${workspaces ? 'model' : visibility}"${middle}, ${destructive.has(id)}, "${workspaces ? workspacesDescription : description}"),${end}`;
    });
  if (rows !== 69) throw new Error(`expected 69 operation rows, rewrote ${rows}`);
  writeFileSync(file, lines.join('\n'));
  process.stdout.write(`rewrote ${rows} rows\n`);
  ```
  Run: `bun .artifacts\cure-contract\c4-table.mjs` → `rewrote 69 rows`. Spot-check three rows
  (the script inserts `"<operator>"` after the alias and `true|false` after the status):
  ```text
  (list_workspaces, $crate::workspace::ListWorkspacesRequest, $crate::workspace::ListWorkspacesResponse, "/api/workspaces/list-workspaces", "Workspaces", "workspaces", "", "model", Read, "", 200, false, "List the workspaces this connection may read, each with its current head revision. Treat head as a pin: pass it to later reads as at = {\"kind\": \"revision\", \"revision\": head} so every call sees one consistent state, and compare it with a later listing to detect change."),
  (archive_workspace, $crate::workspace::ArchiveWorkspaceRequest, $crate::common::MutationResult, "/api/workspaces/archive-workspace", "Archive", "", "", "", Admin, "", 200, true, "Archive without deleting historical content."),
  (log_items, $crate::history::LogRequest, $crate::history::LogResponse, "/api/history/log-items", "Timeline", "log", "timeline", "model", Read, "timeline", 200, false, "Read content snapshots; Git history is not the complete application event log."),
  ```
  `Select-String -Path crates\contract\src\operations.rs -Pattern ', true, "' | Measure-Object`
  → `Count : 7`.
- [ ] `crates/contract/src/operations.rs:1-6` → replace the header with:
  ```rust
  //! Canonical operation declarations consumed by Rust and generated interface adapters.
  //!
  //! Every entry is an application operation, not a handler stub or a second schema language.
  //! Every `$request` implements `scope::RequestScope`; `metadata` enforces that at compile time.
  //!
  //! Columns, in order: id, request type, response type, HTTP path, UI label, agent tool alias
  //! (empty: not an MCP tool), operator and CLI alias (empty: none), tool visibility (`model`,
  //! `app` or empty), minimum permission, MCP App presentation key, HTTP success status,
  //! destructive hint, description. The `permission` column is the minimum capability;
  //! `create_confirmation` requires a stronger, action-specific one through its `targets()`.
  ```
- [ ] `crates/contract/src/metadata.rs:12-30` → replace the macro with:
  ```rust
  macro_rules! collect_operations {
      ($(($id:ident, $request:ty, $response:ty, $path:literal, $label:literal, $alias:literal,
          $operator:literal, $visibility:literal, $permission:ident, $ui:literal, $status:literal,
          $destructive:literal, $description:literal)),* $(,)?) => {
          /// Return canonical operations without opening storage or running an HTTP server.
          #[must_use]
          pub fn operations() -> Vec<OperationInfo> {
              vec![$(OperationInfo {
                  id: stringify!($id), path: $path, label: $label, alias: $alias,
                  operator_alias: $operator, visibility: $visibility,
                  permission: Permission::$permission, ui: $ui, success_status: $status,
                  destructive: $destructive, description: $description,
              }),*]
          }

          const _: () = {
              const fn scoped<T: crate::scope::RequestScope>() {}
              $(scoped::<$request>();)*
          };
      };
  }
  ```
- [ ] `crates/contract/src/metadata.rs:56-77` → replace `OperationInfo` with:
  ```rust
  /// An operation's shared names and declared execution policy.
  #[derive(Debug, Clone, Serialize)]
  pub struct OperationInfo {
      /// Canonical Rust, OpenAPI, and telemetry operation identifier.
      pub id: &'static str,
      /// Declared HTTP endpoint.
      pub path: &'static str,
      /// Human-facing operator vocabulary.
      pub label: &'static str,
      /// Agent tool name; empty means the operation is not an MCP tool.
      pub alias: &'static str,
      /// Operator and CLI name for the same operation; empty means only the canonical id.
      pub operator_alias: &'static str,
      /// Model-facing, app-only, or not exposed through MCP.
      pub visibility: &'static str,
      /// Minimum application capability, independent of HTTP method.
      pub permission: Permission,
      /// Optional approved MCP App presentation key.
      pub ui: &'static str,
      /// HTTP success status.
      pub success_status: u16,
      /// Whether the operation destroys or overwrites user state; a hint, never access control.
      pub destructive: bool,
      /// Shared operation documentation.
      pub description: &'static str,
  }
  ```
- [ ] `crates/contract/src/lib.rs`: `:26` `/// Operation names, labels, and declared policy.` →
  `/// Operation names, aliases, hints and declared policy.`; delete `:49-51` (the blank line,
  the `labels` doc line and `pub mod labels;`). Then `git rm crates/contract/src/labels.rs`.
- [ ] Macro pattern in every consumer. In each file below, replace the two pattern lines
  ```rust
      ($(($id:ident, $request:ty, $response:ty, $path:literal, $label:literal, $alias:literal, $visibility:literal,
          $permission:ident, $ui:literal, $status:literal, $description:literal)),* $(,)?) => {
  ```
  with
  ```rust
      ($(($id:ident, $request:ty, $response:ty, $path:literal, $label:literal, $alias:literal,
          $operator:literal, $visibility:literal, $permission:ident, $ui:literal, $status:literal,
          $destructive:literal, $description:literal)),* $(,)?) => {
  ```
  - `crates/contract/tests/scope.rs:19-20`
  - `crates/core/src/ports.rs:11-12`
  - `crates/core/src/dispatch.rs:36-37`
  - `crates/core/tests/support/counting.rs:13-14`
  - `crates/server/src/lib.rs:34-35`
  - `xtask/src/api.rs:21-22`
  - `tests/support/application.rs:10-11`
- [ ] `xtask/src/api.rs:25-29` → the `OperationInfo` literal gains the two fields:
  ```rust
              $(result.push(typed::<$request, $response>(OperationInfo {
                  id: stringify!($id), path: $path, label: $label, alias: $alias,
                  operator_alias: $operator, visibility: $visibility,
                  permission: Permission::$permission, ui: $ui, success_status: $status,
                  destructive: $destructive, description: $description,
              })?);)*
  ```
- [ ] `xtask/src/api.rs:443` →
  ```rust
          "x-cli-alias":(!operation.info.operator_alias.is_empty()).then_some(operation.info.operator_alias),
  ```
  (same output as before: the alias string, or `null`). Leave the name list at `:450-453`
  alone; package D replaces it with `info.destructive`.
- [ ] `crates/cli/src/lib.rs`: delete `:4` (`use okf_jawn_contract::labels::operator_alias;`);
  replace `:33-37` with:
  ```rust
          if !operation.operator_alias.is_empty() && operation.operator_alias != operation.alias {
              child = child.visible_alias(operation.operator_alias);
          }
  ```
- [ ] Run:
  ```powershell
  cargo fmt -p okf-jawn-contract
  rustfmt --check --edition 2024 crates/cli/src/lib.rs crates/server/src/lib.rs crates/core/src/ports.rs tests/support/application.rs
  cargo test --locked -p okf-jawn-contract -p okf-jawn-core -p okf-jawn-server -p xtask
  cargo check --locked -p okf-jawn-cli -p okf-jawn-mcp --all-targets
  ```
  Expected: `rustfmt --check` prints nothing; all `test result:` lines `ok`; `operations`
  target `6 passed`; `cargo check` finishes without errors.
- [ ] Commit:
  ```powershell
  git add crates/contract/src/operations.rs crates/contract/src/metadata.rs crates/contract/src/lib.rs crates/contract/src/labels.rs crates/contract/tests/operations.rs crates/contract/tests/scope.rs crates/core/src/ports.rs crates/core/src/dispatch.rs crates/core/tests/support/counting.rs crates/server/src/lib.rs crates/cli/src/lib.rs xtask/src/api.rs tests/support/application.rs
  ```
  ```text
  feat(contract): carry operator alias and destructive hint in the operation table.

  Why: the generator and CLI decided both by matching operation ids (labels.rs, xtask name list); agents had no tool to discover workspaces (owner decision "Agent discovery"; SPEC 9 "Generate MCP schemas and annotations from the declared operation semantics").
  What changed: table rows are 13 fields ($operator after $alias, $destructive after $status); OperationInfo gains operator_alias and destructive; labels.rs is deleted; list_workspaces is the model tool "workspaces" with pinning guidance; seven operations are destructive. Every tuple consumer matches the new shape; the CLI reads operator_alias.
  Verified: cargo test --locked -p okf-jawn-contract --test operations failed with E0609 before, 6 passed after; cargo test --locked -p okf-jawn-contract -p okf-jawn-core -p okf-jawn-server -p xtask passes; cargo check --locked -p okf-jawn-cli -p okf-jawn-mcp --all-targets passes.
  Next: Target::Authenticated and ReplayPolicy::AlreadyIssued { id_pointer }.

  Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>
  Claude-Session: https://claude.ai/code/session_01LoZr4ME3crLYjcTNn5GcAC
  ```
- [ ] Mutation check of the two guard tests (no commit). In
  `crates/contract/src/operations.rs`, on the `save_draft` row change `"Save", "", "", "",` to
  `"Save", "save", "", "model",` and run
  `cargo test --locked -p okf-jawn-contract --test operations`. Expected: 
  `only_read_and_propose_operations_are_visible_to_agents` fails with `save_draft is
  agent-visible but requires Write` and `draft_bearing_operations_are_never_agent_tools` fails
  with `save_draft carries drafts and must not have a tool alias`. Then
  `git checkout -- crates/contract/src/operations.rs` and confirm `git status --porcelain` is empty.

---

### Task C.5: `Target::Authenticated`; `ReplayPolicy::AlreadyIssued { id_pointer }`

**Files:**
- Modify: `crates/contract/src/scope.rs:1-6` (header), `:72-83` (macro), `:85-101` (enums),
  `:115-123` (`impl Target`), `:187-193` (macro call), `:271` (`REPLAY`)
- Modify: `crates/contract/tests/scope.rs:7-11` (imports), `:43-47` (constants), `:173-198`,
  `:241-256` (tests)
- Modify (minimum edit): `crates/core/src/dispatch.rs` (two match arms)

**Interfaces:**
- Produces:
  ```rust
  pub enum Target { Authenticated, Deployment(Permission), Workspace(WorkspaceId, Permission) }
  impl Target { pub const fn permission(self) -> Permission; } // Authenticated => Permission::Read
  pub enum ReplayPolicy { StoredResponse, AlreadyIssued { id_pointer: &'static str } }
  // CreateConnectorRequest::REPLAY = ReplayPolicy::AlreadyIssued { id_pointer: "/connector/connector_id" }
  // Empty (get_session), ListWorkspacesRequest, GetCatalogRequest, HealthRequest, ReadinessRequest
  //   -> vec![Target::Authenticated]
  ```

- [ ] Write the failing tests in `crates/contract/tests/scope.rs`.
  - Imports (`:7-11`) →
    ```rust
    use okf_jawn_contract::access::{CreateConnectorRequest, IssuedConnector, Permission};
    use okf_jawn_contract::error::{ApiError, ErrorCode, ErrorDetail};
    use okf_jawn_contract::identity::ConnectorId;
    use okf_jawn_contract::metadata::{OperationName, operations};
    use okf_jawn_contract::scope::{ReplayPolicy, RequestScope, Target};
    use okf_jawn_contract::views::PresentRequest;
    ```
  - After the `ACTION_DEPENDENT` constant add:
    ```rust
    /// Operations any signed-in principal may call; their results are filtered by grants.
    const SIGN_IN_ONLY: &[&str] = &[
        "list_workspaces",
        "get_catalog",
        "get_session",
        "get_health",
        "get_readiness",
    ];
    ```
  - In `every_request_names_its_table_permission_first`, directly after the `let first = …?;`
    statement, insert:
    ```rust
            if SIGN_IN_ONLY.contains(&operation.id) {
                assert_eq!(
                    *first,
                    Target::Authenticated,
                    "{} needs sign-in only",
                    operation.id
                );
            } else {
                assert_ne!(
                    *first,
                    Target::Authenticated,
                    "{} must name a grant target",
                    operation.id
                );
            }
    ```
    (the existing `ACTION_DEPENDENT` branch stays: `create_confirmation` is still exempt from
    the table-permission equality).
  - Replace `only_connector_issuance_refuses_to_replay_its_response` with:
    ```rust
    #[test]
    fn only_connector_issuance_refuses_to_replay_its_response() -> Result<(), Box<dyn Error>> {
        let issued = ReplayPolicy::AlreadyIssued {
            id_pointer: "/connector/connector_id",
        };
        assert_eq!(<CreateConnectorRequest as RequestScope>::REPLAY, issued);
        for operation in sampled_operations()? {
            let expected = if operation.id == "create_connector" {
                issued
            } else {
                ReplayPolicy::StoredResponse
            };
            assert_eq!(operation.replay, expected, "{} replay policy", operation.id);
        }
        Ok(())
    }

    #[test]
    fn the_already_issued_pointer_finds_the_connector_id_in_the_response()
    -> Result<(), Box<dyn Error>> {
        let ReplayPolicy::AlreadyIssued { id_pointer } =
            <CreateConnectorRequest as RequestScope>::REPLAY
        else {
            return Err("create_connector must replay as already_issued".into());
        };
        let response = synthesize_for::<IssuedConnector>()?;
        let issued: IssuedConnector = serde_json::from_value(response.clone())?;
        let found = response
            .pointer(id_pointer)
            .cloned()
            .ok_or_else(|| format!("{id_pointer} is absent from IssuedConnector"))?;
        let connector_id: ConnectorId = serde_json::from_value(found)?;
        assert_eq!(connector_id, issued.connector.connector_id);
        Ok(())
    }

    #[test]
    fn tenant_level_reads_need_sign_in_only() -> Result<(), Box<dyn Error>> {
        assert_eq!(Target::Authenticated.permission(), Permission::Read);
        let mut found = Vec::new();
        for operation in sampled_operations()? {
            if operation.targets.contains(&Target::Authenticated) {
                assert_eq!(
                    operation.targets,
                    vec![Target::Authenticated],
                    "{} mixes sign-in with a grant target",
                    operation.id
                );
                assert_eq!(operation.permission, Permission::Read, "{}", operation.id);
                assert!(!operation.keyed, "{} is a read", operation.id);
                found.push(operation.id);
            }
        }
        found.sort_unstable();
        let mut expected = SIGN_IN_ONLY.to_vec();
        expected.sort_unstable();
        assert_eq!(found, expected);
        Ok(())
    }
    ```
- [ ] Run: `cargo test --locked -p okf-jawn-contract --test scope`
  Expected failure: `error[E0599]: no variant or associated item named `Authenticated` found
  for enum `Target`` and `error[E0559]: variant `ReplayPolicy::AlreadyIssued` has no field
  named `id_pointer``.
- [ ] Implement in `crates/contract/src/scope.rs`:
  - `:1-6` →
    ```rust
    //! Every request declares its authorization targets and retry identity in its type.
    //!
    //! Dispatch authorizes every target before the handler runs; a request that touches several
    //! workspaces names each one, so no workspace is reached without its own grant. The first
    //! target carries the operation's table permission, except `create_confirmation`, whose
    //! required permission depends on the confirmed action. `Target::Authenticated` asks only
    //! for a signed-in principal: session, workspace listing, catalog and health results are
    //! filtered by the caller's grants instead of being gated by one.
    ```
  - `:72-83` (`macro_rules! deployment_read`) →
    ```rust
    macro_rules! authenticated {
        ($($request:ty),* $(,)?) => {
            $(impl RequestScope for $request {
                fn targets(&self) -> Vec<Target> {
                    vec![Target::Authenticated]
                }
                fn idempotency_key(&self) -> Option<&IdempotencyKey> {
                    None
                }
            })*
        };
    }
    ```
  - `:85-101` (both enums) →
    ```rust
    /// A resource the caller must hold a permission on before the handler runs.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
    pub enum Target {
        /// Any signed-in principal; no grant lookup. The handler filters results by grants.
        Authenticated,
        /// The caller's tenant within this deployment, checked through the tenant grant.
        Deployment(Permission),
        /// One workspace, checked through that workspace's grant.
        Workspace(WorkspaceId, Permission),
    }

    /// What a replay of a completed mutation under the same key returns.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
    pub enum ReplayPolicy {
        /// Return the stored response unchanged.
        StoredResponse,
        /// Return `already_issued` with the created identity; the response held a secret that is
        /// never stored or replayed.
        AlreadyIssued {
            /// JSON pointer into the response to the created identity.
            id_pointer: &'static str,
        },
    }
    ```
  - `:115-123` (`impl Target`) →
    ```rust
    impl Target {
        /// The capability this target requires; sign-in alone is reported as `Read`.
        #[must_use]
        pub const fn permission(self) -> Permission {
            match self {
                Self::Authenticated => Permission::Read,
                Self::Deployment(permission) | Self::Workspace(_, permission) => permission,
            }
        }
    }
    ```
  - `:187-193` → `deployment_read!(` becomes `authenticated!(` (the five type names stay).
  - `:271` →
    ```rust
        const REPLAY: ReplayPolicy = ReplayPolicy::AlreadyIssued {
            id_pointer: "/connector/connector_id",
        };
    ```
- [ ] Implement the two minimum edits in `crates/core/src/dispatch.rs`:
  - in `fn replay_response`: `ReplayPolicy::AlreadyIssued => {` →
    `ReplayPolicy::AlreadyIssued { .. } => {`
  - in `fn authorize_targets`, first arm of `match target {`:
    ```rust
                Target::Authenticated => {}
    ```
  Behaviour of every existing path is unchanged; package E owns what dispatch does with
  `id_pointer` and with a context that has no grant.
- [ ] Run:
  ```powershell
  cargo fmt -p okf-jawn-contract
  cargo test --locked -p okf-jawn-contract -p okf-jawn-core -p okf-jawn-server -p xtask
  ```
  Expected: all `ok`.
- [ ] Commit `git add crates/contract/src/scope.rs crates/contract/tests/scope.rs crates/core/src/dispatch.rs`:
  ```text
  feat(contract): add Target::Authenticated and a pointer on ReplayPolicy::AlreadyIssued.

  Why: session, workspace listing, catalog and health are tenant-level reads any signed-in principal may make (engineering decision "Tenant-level reads"); dispatch stripped connector secrets by matching the operation name instead of a declared policy.
  What changed: Target gains Authenticated (permission() reports Read); get_session, list_workspaces, get_catalog, get_health and get_readiness target it; ReplayPolicy::AlreadyIssued carries id_pointer "/connector/connector_id". dispatch.rs gets the two match arms needed to compile.
  Verified: cargo test --locked -p okf-jawn-contract --test scope failed with E0599/E0559 before and passes after; cargo test --locked -p okf-jawn-contract -p okf-jawn-core -p okf-jawn-server -p xtask passes.
  Next: one wire shape per type.

  Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>
  Claude-Session: https://claude.ai/code/session_01LoZr4ME3crLYjcTNn5GcAC
  ```

---

### Task C.6: One wire shape per type (symmetric `Option` fields)

**Files:**
- Create: `crates/contract/tests/schema.rs`
- Modify: `crates/contract/src/history.rs:77-84`, `crates/contract/src/source.rs:81-82`

**Interfaces:**
- Produces: `FileChange.old_path`, `FileChange.new_path`, `GetSourcesResponse.appearance` carry
  `#[serde(default, skip_serializing_if = "Option::is_none")]`; `FileChange` derives
  `PartialEq, Eq`.
- Guarantee package D relies on: for every `$request` and `$response` in `for_each_operation!`,
  and for `ApiError`, `ResourceMetadata`, `NamingRules`, `ViewDocument`, `TypeDefinition`, the
  schemars deserialize and serialize schemas are identical.
- Why these three: schemars_derive 1.2.2 `schema_exprs.rs:730-739` makes a field optional in
  both contracts only when it has `default` and `skip_serializing_if`; a bare `Option` is
  optional only under `contract().is_deserialize()`. `api/schemars-splits.json` at base lists
  exactly `GetSourcesResponse`, `FileChange` and (through its `$ref`) `DiffResponse`; every other
  `Option` field in the crate already has both attributes.

- [ ] Write the failing test. Create `crates/contract/tests/schema.rs`:
  ```rust
  //! Every wire type has one shape: its serialize and deserialize schemas are identical.
  //!
  //! Package D's generator refuses a type with two shapes; this is the contract-side guard.

  use std::error::Error;

  use okf_jawn_contract::access::ResourceMetadata;
  use okf_jawn_contract::conventions::NamingRules;
  use okf_jawn_contract::error::ApiError;
  use okf_jawn_contract::item::TypeDefinition;
  use okf_jawn_contract::views::ViewDocument;
  use schemars::{JsonSchema, generate::SchemaSettings};
  use serde::{Deserialize, Serialize};
  use serde_json::{Map, Value};

  macro_rules! schema_checks {
      ($(($id:ident, $request:ty, $response:ty, $path:literal, $label:literal, $alias:literal,
          $operator:literal, $visibility:literal, $permission:ident, $ui:literal, $status:literal,
          $destructive:literal, $description:literal)),* $(,)?) => {
          fn split_operation_types() -> Result<Vec<String>, Box<dyn Error>> {
              let mut split = Vec::new();
              $(
                  record_split::<$request>(concat!(stringify!($id), " request"), &mut split)?;
                  record_split::<$response>(concat!(stringify!($id), " response"), &mut split)?;
              )*
              Ok(split)
          }
      };
  }

  /// Probe: a bare `Option` is optional when read and required when written.
  #[derive(Serialize, Deserialize, JsonSchema)]
  struct Asymmetric {
      note: Option<String>,
  }

  fn schema_for<T: JsonSchema>(serialize: bool) -> Result<Value, serde_json::Error> {
      let settings = SchemaSettings::draft2020_12();
      let settings = if serialize {
          settings.for_serialize()
      } else {
          settings.for_deserialize()
      };
      serde_json::to_value(settings.into_generator().into_root_schema_for::<T>())
  }

  fn without_definitions(schema: &Value) -> Value {
      let mut body = schema.clone();
      if let Some(object) = body.as_object_mut() {
          object.remove("$defs");
      }
      body
  }

  /// Names of the definitions whose two schemas differ, and the root when its own body differs.
  fn differing(root: &str, deserialize: &Value, serialize: &Value) -> Vec<String> {
      let empty = Map::new();
      let before = deserialize
          .get("$defs")
          .and_then(Value::as_object)
          .unwrap_or(&empty);
      let after = serialize
          .get("$defs")
          .and_then(Value::as_object)
          .unwrap_or(&empty);
      let mut names: Vec<String> = before
          .keys()
          .chain(after.keys())
          .filter(|key| before.get(*key) != after.get(*key))
          .cloned()
          .collect();
      names.sort();
      names.dedup();
      if without_definitions(deserialize) != without_definitions(serialize) {
          names.push(root.to_owned());
      }
      names
  }

  fn record_split<T: JsonSchema>(role: &str, split: &mut Vec<String>) -> Result<(), Box<dyn Error>> {
      let deserialize = schema_for::<T>(false)?;
      let serialize = schema_for::<T>(true)?;
      if deserialize != serialize {
          let root = T::schema_name();
          split.push(format!(
              "{role} ({})",
              differing(&root, &deserialize, &serialize).join(", ")
          ));
      }
      Ok(())
  }

  okf_jawn_contract::for_each_operation!(schema_checks);

  #[test]
  fn every_request_and_response_has_one_wire_shape() -> Result<(), Box<dyn Error>> {
      let mut split = split_operation_types()?;
      record_split::<ApiError>("ApiError", &mut split)?;
      record_split::<ResourceMetadata>("ResourceMetadata", &mut split)?;
      record_split::<NamingRules>("NamingRules", &mut split)?;
      record_split::<ViewDocument>("ViewDocument", &mut split)?;
      record_split::<TypeDefinition>("TypeDefinition", &mut split)?;
      assert!(
          split.is_empty(),
          "serialize and deserialize schemas differ for: {}",
          split.join("; ")
      );
      Ok(())
  }

  #[test]
  fn a_field_optional_only_when_read_is_reported_by_type_name() -> Result<(), Box<dyn Error>> {
      let mut split = Vec::new();
      record_split::<Asymmetric>("probe", &mut split)?;
      assert_eq!(split, ["probe (Asymmetric)"]);
      Ok(())
  }
  ```
- [ ] Run: `cargo test --locked -p okf-jawn-contract --test schema`
  Expected: `a_field_optional_only_when_read_is_reported_by_type_name ... ok` (the detector
  works) and `every_request_and_response_has_one_wire_shape ... FAILED` with
  `serialize and deserialize schemas differ for: get_sources response (GetSourcesResponse);
  diff_items response (FileChange)`. If the message names any other type, apply the same cure
  to it in this task and add it to the commit message.
- [ ] Implement in `crates/contract/src/history.rs:77-84` →
  ```rust
  /// A changed file with before and after locators.
  #[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
  #[serde(deny_unknown_fields)]
  pub struct FileChange {
      /// Previous path; absent for additions.
      #[serde(default, skip_serializing_if = "Option::is_none")]
      pub old_path: Option<String>,
      /// New path; absent for deletions.
      #[serde(default, skip_serializing_if = "Option::is_none")]
      pub new_path: Option<String>,
  ```
  (the remaining three fields are unchanged).
- [ ] Implement in `crates/contract/src/source.rs:81-82` →
  ```rust
      /// Occurrence metadata if the item is a source.
      #[serde(default, skip_serializing_if = "Option::is_none")]
      pub appearance: Option<SourceAppearance>,
  ```
- [ ] Run:
  ```powershell
  cargo fmt -p okf-jawn-contract
  cargo test --locked -p okf-jawn-contract -p okf-jawn-core -p okf-jawn-server -p xtask
  ```
  Expected: all `ok`; `schema` target `2 passed`.
- [ ] Commit `git add crates/contract/src/history.rs crates/contract/src/source.rs crates/contract/tests/schema.rs`:
  ```text
  fix(contract): give FileChange and GetSourcesResponse one wire shape.

  Why: three bare Option fields were optional when read and required when written, so the generator emitted Input/Output pairs (api/schemars-splits.json) and the UI had to pick one (SPEC 14: "test omitted/null ... semantically across boundaries").
  What changed: FileChange.old_path/new_path and GetSourcesResponse.appearance are omitted when absent and optional when read; FileChange derives PartialEq and Eq. New test compares the serialize and deserialize schemas of every request, response and shared wire type.
  Verified: cargo test --locked -p okf-jawn-contract --test schema failed naming GetSourcesResponse and FileChange before, 2 passed after; cargo test --locked -p okf-jawn-contract -p okf-jawn-core -p okf-jawn-server -p xtask passes.
  Next: typed draft conflicts and the Snapshot precondition.

  Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>
  Claude-Session: https://claude.ai/code/session_01LoZr4ME3crLYjcTNn5GcAC
  ```

---

### Task C.7: Typed draft conflicts; `CommitRequest` without `expected_head`

**Files:**
- Modify: `crates/contract/src/error.rs:39-71` (`ErrorDetail`, new `DraftConflictItem`)
- Modify: `crates/contract/src/history.rs:107-125` (`CommitRequest`)
- Modify: `crates/contract/src/operations.rs` (the `commit_items` row, description only)
- Modify: `crates/contract/tests/error.rs`, `crates/contract/tests/semantic.rs`,
  `crates/contract/tests/operations.rs`

**Interfaces:**
- Produces:
  ```rust
  pub struct DraftConflictItem {
      pub item_id: ItemId,
      pub draft_base: Revision,
      pub current_revision: Revision,
      pub deleted: bool,
      pub changes: Vec<crate::history::FileChange>,
  }
  // ErrorDetail::DraftConflict { items: Vec<DraftConflictItem> }
  pub struct CommitRequest { pub workspace_id, pub item_ids /* min 1 */, pub message, pub idempotency_key }
  ```
- Wire: `{"kind":"draft_conflict","items":[{"item_id":…,"draft_base":…,"current_revision":…,
  "deleted":false,"changes":[FileChange…]}]}`.

- [ ] Write the failing tests.
  - `crates/contract/tests/error.rs`: add
    `use okf_jawn_contract::history::FileChangeKind;` after the `error` import, and append:
    ```rust
    #[test]
    fn a_draft_conflict_lists_every_conflicting_item_with_typed_changes()
    -> Result<(), Box<dyn Error>> {
        let wire = json!({
            "code": "conflict",
            "message": "Two selected items changed after their drafts were based",
            "detail": {
                "kind": "draft_conflict",
                "items": [
                    {
                        "item_id": "22222222-2222-4222-8222-222222222222",
                        "draft_base": "a".repeat(40),
                        "current_revision": "b".repeat(40),
                        "deleted": false,
                        "changes": [{
                            "old_path": "notes/a.md",
                            "new_path": "notes/a.md",
                            "patch": "@@ -1 +1 @@\n-old\n+new\n",
                            "binary": false,
                            "kind": "modified"
                        }]
                    },
                    {
                        "item_id": "33333333-3333-4333-8333-333333333333",
                        "draft_base": "a".repeat(40),
                        "current_revision": "b".repeat(40),
                        "deleted": true,
                        "changes": [{
                            "old_path": "notes/b.md",
                            "patch": "",
                            "binary": false,
                            "kind": "removed"
                        }]
                    }
                ]
            }
        });
        let error: ApiError = serde_json::from_value(wire.clone())?;
        let Some(ErrorDetail::DraftConflict { items }) = error.detail.as_deref() else {
            return Err("expected a draft_conflict detail".into());
        };
        let deleted: Vec<bool> = items.iter().map(|item| item.deleted).collect();
        assert_eq!(deleted, [false, true]);
        let kinds: Vec<&FileChangeKind> = items
            .iter()
            .flat_map(|item| &item.changes)
            .map(|change| &change.kind)
            .collect();
        assert_eq!(kinds, [&FileChangeKind::Modified, &FileChangeKind::Removed]);
        assert_eq!(serde_json::to_value(&error)?, wire);
        Ok(())
    }

    #[test]
    fn an_untyped_diff_is_not_a_draft_conflict() {
        let old = json!({
            "kind": "draft_conflict",
            "item_id": "22222222-2222-4222-8222-222222222222",
            "draft_base": "a".repeat(40),
            "current_revision": "b".repeat(40),
            "diff": {"anything": true}
        });
        assert!(serde_json::from_value::<ErrorDetail>(old).is_err());
    }
    ```
  - `crates/contract/tests/semantic.rs`: add `use okf_jawn_contract::history::CommitRequest;`
    as the first import, and append:
    ```rust
    #[test]
    fn a_snapshot_request_carries_no_expected_head() -> Result<(), Box<dyn Error>> {
        let request = json!({
            "workspace_id": "11111111-1111-4111-8111-111111111111",
            "item_ids": ["22222222-2222-4222-8222-222222222222"],
            "message": "Quarterly numbers",
            "idempotency_key": "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa"
        });
        let decoded: CommitRequest = serde_json::from_value(request.clone())?;
        assert_eq!(decoded.item_ids.len(), 1);
        let mut with_head = request;
        with_head
            .as_object_mut()
            .ok_or("request must be an object")?
            .insert("expected_head".to_owned(), json!("a".repeat(40)));
        assert!(
            serde_json::from_value::<CommitRequest>(with_head).is_err(),
            "head movement alone is not a Snapshot precondition"
        );
        Ok(())
    }
    ```
  - `crates/contract/tests/operations.rs`: append
    ```rust
    #[test]
    fn snapshot_states_the_per_item_precondition() -> Result<(), Box<dyn Error>> {
        let commit = declared("commit_items")?;
        assert!(commit.description.contains("since its draft's base"));
        assert!(!commit.description.contains("unchanged base"));
        Ok(())
    }
    ```
- [ ] Run: `cargo test --locked -p okf-jawn-contract`
  Expected failures: `error` target does not compile
  (`error[E0026]: variant `ErrorDetail::DraftConflict` does not have a field named `items``);
  `semantic` → `a_snapshot_request_carries_no_expected_head` fails with
  `missing field `expected_head``; `operations` → `snapshot_states_the_per_item_precondition`
  fails on the first assertion.
- [ ] Implement in `crates/contract/src/error.rs`. Replace the `DraftConflict` variant
  (`:55-65`) with:
  ```rust
      /// One or more snapshotted items changed or were deleted after their drafts' bases.
      DraftConflict {
          /// Every conflicting item, never only the first.
          #[schemars(length(min = 1))]
          items: Vec<DraftConflictItem>,
      },
  ```
  change the derive on `ErrorDetail` (`:40`) to
  `#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]`, and insert
  between `ErrorDetail` and `ApiError`:
  ```rust
  /// One snapshotted item whose committed content moved after its draft's base.
  #[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
  #[serde(deny_unknown_fields)]
  pub struct DraftConflictItem {
      /// Item whose draft no longer applies cleanly.
      pub item_id: crate::identity::ItemId,
      /// Revision the draft was based on.
      pub draft_base: crate::identity::Revision,
      /// Head revision at which the item was found changed or deleted.
      pub current_revision: crate::identity::Revision,
      /// The item no longer exists at `current_revision`.
      pub deleted: bool,
      /// The item's committed changes from `draft_base` to `current_revision`.
      pub changes: Vec<crate::history::FileChange>,
  }
  ```
- [ ] Implement in `crates/contract/src/history.rs:107-125` →
  ```rust
  /// Snapshot the caller's drafts of the selected items in one commit.
  ///
  /// Each draft's own base revision is the precondition. The Snapshot is blocked only when an
  /// item being snapshotted was itself changed or deleted since its draft's base; head movement
  /// that did not touch a selected item never blocks. When blocked, nothing is committed and the
  /// `draft_conflict` error detail lists every conflicting item, not only the first. On success
  /// the snapshotted drafts are removed.
  #[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
  #[serde(deny_unknown_fields)]
  pub struct CommitRequest {
      /// Workspace whose permissions and storage scope apply.
      pub workspace_id: crate::identity::WorkspaceId,
      /// Items whose drafts by the caller are snapshotted together.
      #[schemars(length(min = 1))]
      pub item_ids: Vec<crate::identity::ItemId>,
      /// Human-readable snapshot name.
      pub message: String,
      /// Retry identity.
      pub idempotency_key: crate::identity::IdempotencyKey,
  }
  ```
- [ ] Implement in `crates/contract/src/operations.rs`, `commit_items` row: the description
  `"Name a snapshot from saved drafts against an unchanged base."` →
  `"Name a snapshot from the caller's saved drafts; blocked only when a selected item was changed or deleted since its draft's base."`
- [ ] Run:
  ```powershell
  cargo fmt -p okf-jawn-contract
  cargo test --locked -p okf-jawn-contract -p okf-jawn-core -p okf-jawn-server -p xtask
  ```
  Expected: all `ok` (including `schema`: the new types have one wire shape).
- [ ] Commit `git add crates/contract/src/error.rs crates/contract/src/history.rs crates/contract/src/operations.rs crates/contract/tests/error.rs crates/contract/tests/semantic.rs crates/contract/tests/operations.rs`:
  ```text
  feat(contract): report every conflicting draft with a typed diff; drop expected_head.

  Why: owner decision "Snapshot conflict": a Snapshot is blocked only when an item being snapshotted was itself changed or deleted since its draft's base; unrelated head movement never blocks; the error lists every conflicting item with its diff. The old detail carried one item and an untyped diff, and CommitRequest rejected on any head movement.
  What changed: ErrorDetail::DraftConflict { items: Vec<DraftConflictItem> } with typed Vec<FileChange>; CommitRequest loses expected_head and documents the rule; commit_items description states it.
  Verified: cargo test --locked -p okf-jawn-contract failed (E0026, "missing field `expected_head`") before and passes after; cargo test --locked -p okf-jawn-contract -p okf-jawn-core -p okf-jawn-server -p xtask passes.
  Next: JobKind. Blocked for SPEC text only: SPEC.md section 8 still says "if the head moved under a draft, the whole commit is rejected" (integration owner).

  Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>
  Claude-Session: https://claude.ai/code/session_01LoZr4ME3crLYjcTNn5GcAC
  ```

---

### Task C.8: `JobKind` and `Job.kind`

**Files:** Modify `crates/contract/src/import.rs:6-49`, `crates/contract/tests/semantic.rs`.

**Interfaces:** Produces
```rust
pub enum JobKind { Import, Redigest, ExportWorkspace, BackupWorkspace, RestoreWorkspace, RebuildIndex, ExportView }
// Job gains: pub kind: JobKind   (required; wire snake_case)
```

- [ ] Write the failing test in `crates/contract/tests/semantic.rs`. Imports become:
  ```rust
  use okf_jawn_contract::history::CommitRequest;
  use okf_jawn_contract::identity::{At, Digest, Revision, WorkspacePath};
  use okf_jawn_contract::import::{Job, JobKind};
  use okf_jawn_contract::metadata::operations;
  use okf_jawn_contract::read::ReadItemRequest;
  use okf_jawn_contract::views::ViewDocument;
  use serde_json::json;
  use std::error::Error;
  use std::fs;
  use std::path::PathBuf;
  ```
  After the imports add:
  ```rust
  /// Wire names of every job kind, one per operation that starts a job.
  const JOB_KINDS: &[&str] = &[
      "import",
      "redigest",
      "export_workspace",
      "backup_workspace",
      "restore_workspace",
      "rebuild_index",
      "export_view",
  ];
  ```
  Append:
  ```rust
  #[test]
  fn every_job_states_its_kind() -> Result<(), Box<dyn Error>> {
      let mut job = json!({
          "id": "66666666-6666-4666-8666-666666666666",
          "workspace_id": "11111111-1111-4111-8111-111111111111",
          "kind": "import",
          "state": "queued",
          "progress": 0,
          "attempt": 1,
          "warnings": [],
          "item_ids": []
      });
      let decoded: Job = serde_json::from_value(job.clone())?;
      assert_eq!(decoded.kind, JobKind::Import);
      for kind in JOB_KINDS {
          *job.get_mut("kind").ok_or("kind field")? = json!(kind);
          let decoded: Job = serde_json::from_value(job.clone())?;
          assert_eq!(serde_json::to_value(decoded.kind)?, json!(kind));
      }
      job.as_object_mut()
          .ok_or("job must be an object")?
          .remove("kind");
      assert!(
          serde_json::from_value::<Job>(job).is_err(),
          "a job without a kind must not decode"
      );
      Ok(())
  }

  #[test]
  fn every_operation_that_starts_a_job_has_a_kind() {
      let starters: Vec<&str> = operations()
          .iter()
          .filter(|operation| operation.success_status == 202 && operation.id != "retry_job")
          .map(|operation| operation.id)
          .collect();
      assert_eq!(starters.len(), JOB_KINDS.len(), "202 operations: {starters:?}");
  }
  ```
- [ ] Run: `cargo test --locked -p okf-jawn-contract --test semantic`
  Expected failure: `error[E0432]: unresolved import `okf_jawn_contract::import::JobKind``.
- [ ] Implement in `crates/contract/src/import.rs`. Insert after `JobState` (after `:20`):
  ```rust

  /// What a durable job does; fixed when the job is accepted.
  #[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
  #[serde(rename_all = "snake_case")]
  pub enum JobKind {
      /// Convert finalized uploads into source cards.
      Import,
      /// Re-run extraction for one item.
      Redigest,
      /// Build a portable export.
      ExportWorkspace,
      /// Back up content and durable application records.
      BackupWorkspace,
      /// Restore content and durable application records from a backup.
      RestoreWorkspace,
      /// Rebuild derived search and link data.
      RebuildIndex,
      /// Export one View with its sources, data table and rendering.
      ExportView,
  }
  ```
  In `Job`, after the `workspace_id` field (`:28-29`) add:
  ```rust
      /// What this job does.
      pub kind: JobKind,
  ```
- [ ] Run:
  ```powershell
  cargo fmt -p okf-jawn-contract
  cargo test --locked -p okf-jawn-contract -p okf-jawn-core -p okf-jawn-server -p xtask
  ```
  Expected: all `ok`.
- [ ] Commit `git add crates/contract/src/import.rs crates/contract/tests/semantic.rs`:
  ```text
  feat(contract): give every Job a kind.

  Why: a claimed job could not say what work it is; package F's job specification and the storage lane need the kind on the wire record (design section 5, C: "Job gains a kind").
  What changed: new JobKind (import, redigest, export_workspace, backup_workspace, restore_workspace, rebuild_index, export_view); Job.kind is required.
  Verified: cargo test --locked -p okf-jawn-contract --test semantic failed with E0432 before and passes after; cargo test --locked -p okf-jawn-contract -p okf-jawn-core -p okf-jawn-server -p xtask passes.
  Next: ViewDocument::bindings_outside.

  Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>
  Claude-Session: https://claude.ai/code/session_01LoZr4ME3crLYjcTNn5GcAC
  ```

---

### Task C.9: `ViewDocument::bindings_outside` and the same-workspace rule

**Files:** Modify `crates/contract/src/views.rs:26-41`, `:66-76`, end of file;
`crates/contract/tests/semantic.rs`.

**Interfaces:** Produces
```rust
impl ViewDocument {
    /// Bindings whose source lives outside `workspace`; a saved or presented View must have none.
    pub fn bindings_outside(&self, workspace: WorkspaceId) -> Vec<&ViewBinding>;
}
```
`PresentRequest::targets()` is unchanged (every binding's workspace stays an authorization target).

- [ ] Write the failing test in `crates/contract/tests/semantic.rs`. Change the identity import
  to `use okf_jawn_contract::identity::{At, Digest, Revision, WorkspaceId, WorkspacePath};`,
  add this helper after the `JOB_KINDS` constant:
  ```rust
  fn binding(name: &str, workspace: &str) -> serde_json::Value {
      json!({
          "name": name,
          "source": {
              "workspace_id": workspace,
              "item_id": "22222222-2222-4222-8222-222222222222",
              "path": "data/metrics.csv",
              "revision": "a".repeat(40),
              "selection": {"kind": "all"}
          },
          "units": {},
          "transforms": []
      })
  }
  ```
  and append:
  ```rust
  #[test]
  fn a_view_reports_the_bindings_outside_its_workspace() -> Result<(), Box<dyn Error>> {
      let home = "11111111-1111-4111-8111-111111111111";
      let elsewhere = "33333333-3333-4333-8333-333333333333";
      let view: ViewDocument = serde_json::from_value(json!({
          "schema_version": 1,
          "title": "Metrics",
          "description": "Quarterly metrics",
          "mode": "pinned",
          "grammar": "vega_lite",
          "bindings": [binding("local", home), binding("foreign", elsewhere)],
          "spec": {}
      }))?;
      let workspace: WorkspaceId = serde_json::from_value(json!(home))?;
      let outside: Vec<&str> = view
          .bindings_outside(workspace)
          .into_iter()
          .map(|found| found.name.as_str())
          .collect();
      assert_eq!(outside, ["foreign"]);
      let other: WorkspaceId = serde_json::from_value(json!(elsewhere))?;
      let outside_other: Vec<&str> = view
          .bindings_outside(other)
          .into_iter()
          .map(|found| found.name.as_str())
          .collect();
      assert_eq!(outside_other, ["local"]);
      Ok(())
  }
  ```
- [ ] Run: `cargo test --locked -p okf-jawn-contract --test semantic`
  Expected failure: `error[E0599]: no method named `bindings_outside` found for struct
  `ViewDocument``.
- [ ] Implement in `crates/contract/src/views.rs`:
  - `:26` doc of `ViewBinding` →
    ```rust
    /// A named source selection used by a chart or layout.
    ///
    /// A saved View binds only to sources in its own workspace: `source.workspace_id` must equal
    /// the workspace the View is saved or presented in (`ViewDocument::bindings_outside`).
    ```
  - `:66-68` doc of `PresentRequest` →
    ```rust
    /// Render a candidate from already resolved bindings without saving or approving it.
    ///
    /// Every binding must name `workspace_id`; the handler rejects a view for which
    /// `view.bindings_outside(workspace_id)` is not empty. Each binding's source workspace is
    /// still an authorization target, so a foreign binding is refused before the handler runs.
    ```
  - append at the end of the file:
    ```rust

    impl ViewDocument {
        /// Bindings whose source lives outside `workspace`; a saved or presented View must have none.
        #[must_use]
        pub fn bindings_outside(&self, workspace: crate::identity::WorkspaceId) -> Vec<&ViewBinding> {
            self.bindings
                .iter()
                .filter(|binding| binding.source.workspace_id != workspace)
                .collect()
        }
    }
    ```
- [ ] Run:
  ```powershell
  cargo fmt -p okf-jawn-contract
  cargo test --locked -p okf-jawn-contract -p okf-jawn-core -p okf-jawn-server -p xtask
  ```
  Expected: all `ok` (`present_view_authorizes_every_binding_workspace` still passes).
- [ ] Commit `git add crates/contract/src/views.rs crates/contract/tests/semantic.rs`:
  ```text
  feat(contract): state and expose the same-workspace rule for View bindings.

  Why: owner decision "Saved Views": a saved View binds only to sources in its own workspace; nothing in the contract said so or let a handler check it.
  What changed: ViewDocument::bindings_outside(workspace) returns the offending bindings; ViewBinding and PresentRequest document the rule. PresentRequest::targets() is unchanged.
  Verified: cargo test --locked -p okf-jawn-contract --test semantic failed with E0599 before and passes after; cargo test --locked -p okf-jawn-contract -p okf-jawn-core -p okf-jawn-server -p xtask passes.
  Next: WorkspacePath schema pattern.

  Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>
  Claude-Session: https://claude.ai/code/session_01LoZr4ME3crLYjcTNn5GcAC
  ```

---

### Task C.10: `WorkspacePath` schema carries the expressible part of its rule

**Files:**
- Modify: `crates/contract/src/identity.rs:116-117` (new constant), `:138-140` (schema)
- Modify: `crates/contract/tests/semantic.rs`
- Modify (test only; see Deviations 1): `xtask/src/api.rs` `mod tests`

**Interfaces:**
- Produces: the `WorkspacePath` JSON Schema is
  `{"type":"string","minLength":1,"maxLength":4096,"pattern":P}` where `P` is the 40-character
  string `^[^/\\:\x00-\x1f]+(/[^/\\:\x00-\x1f]+)*$` (two backslash characters before each
  colon; `\x00` and `\x1f` are four characters each).
- Unchanged: `.`, `..` and `.git` segments, DEL and C1 controls, and the byte length are
  rejected by `TryFrom<String>` only.
- Pattern syntax evidence: `jsonschema` is `0.58.5` in `Cargo.lock` (default features off; regex
  backend `fancy-regex 0.19.2`). `jsonschema-regex-0.58.5/src/lib.rs:23-85` (`to_rust_regex`)
  parses the pattern with `regex_syntax::ast::parse::Parser`, which accepts `\xHH`, `\\` and an
  unescaped `/`; `syntax.rs:377-381` accepts `\x` followed by two hex digits as ECMA-262. The
  Hey API Zod plugin emits it as `/^[^\/\\:\x00-\x1f]+(\/[^\/\\:\x00-\x1f]+)*$/` (checked
  against the installed 0.99.0 while planning; it type-checks and rejects `a\b`).

- [ ] Write the failing tests.
  - `crates/contract/tests/semantic.rs`: add `use schemars::generate::SchemaSettings;` before
    the `serde_json` import, and append:
    ```rust
    #[test]
    fn workspace_path_schema_states_the_expressible_part_of_its_rule()
    -> Result<(), Box<dyn Error>> {
        let schema = serde_json::to_value(
            SchemaSettings::draft2020_12()
                .into_generator()
                .into_root_schema_for::<WorkspacePath>(),
        )?;
        assert_eq!(
            schema.get("pattern"),
            Some(&json!(r"^[^/\\:\x00-\x1f]+(/[^/\\:\x00-\x1f]+)*$"))
        );
        assert_eq!(schema.get("minLength"), Some(&json!(1)));
        assert_eq!(schema.get("maxLength"), Some(&json!(4096)));
        // What a regular expression cannot state stays in `TryFrom`.
        for path in [
            ".",
            "..",
            "a/./b",
            "a/../b",
            ".git/config",
            "notes/.GIT/x",
            "a\u{7f}b",
        ] {
            assert!(WorkspacePath::try_from(path.to_owned()).is_err(), "{path:?}");
        }
        Ok(())
    }
    ```
  - `xtask/src/api.rs`, inside `mod tests`, after the existing test:
    ```rust

        #[test]
        fn workspace_path_schema_accepts_and_rejects_through_the_generated_schema()
        -> Result<(), Box<dyn Error>> {
            let directory = tempfile::tempdir()?;
            super::generate(directory.path())?;
            let schema: Value = serde_json::from_slice(&std::fs::read(
                directory
                    .path()
                    .join("schemas")
                    .join("create_item.input.json"),
            )?)?;
            let validator = jsonschema::validator_for(&schema)?;
            let request = |path: &str| {
                json!({
                    "workspace_id": "11111111-1111-4111-8111-111111111111",
                    "base_revision": "a".repeat(40),
                    "path": path,
                    "title": "Note",
                    "type_name": "note",
                    "kind": "note",
                    "body": "",
                    "properties": {},
                    "idempotency_key": "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa"
                })
            };
            for path in ["Clients/one.md", "a", "a/b/c"] {
                assert!(validator.is_valid(&request(path)), "{path:?} must be accepted");
            }
            for path in ["", "/client", "a//b", "a/", "C:/docs", "a\\b", "a\nb"] {
                assert!(!validator.is_valid(&request(path)), "{path:?} must be rejected");
            }
            Ok(())
        }
    ```
- [ ] Run: `cargo test --locked -p okf-jawn-contract --test semantic` → expected failure
  `left: None` on the `pattern` assertion. Run: `cargo test --locked -p xtask` → expected
  failure `"/client" must be rejected`.
- [ ] Implement in `crates/contract/src/identity.rs`. Insert between `IdentityError`
  (ends `:116`) and the first `impl` (`:118`):
  ```rust

  /// The part of the workspace-path rule a regular expression can state: `/`-separated
  /// segments, none empty, none containing `\`, `:` or a C0 control. The `.`, `..` and `.git`
  /// segment rules, DEL and C1 controls, and the byte length are enforced by `TryFrom<String>`.
  const WORKSPACE_PATH_PATTERN: &str = r"^[^/\\:\x00-\x1f]+(/[^/\\:\x00-\x1f]+)*$";
  ```
  and replace the `json_schema!` line in `impl JsonSchema for WorkspacePath` (`:139`) with:
  ```rust
          schemars::json_schema!({
              "type": "string",
              "minLength": 1,
              "maxLength": 4096,
              "pattern": WORKSPACE_PATH_PATTERN
          })
  ```
  (`json_schema!` forwards to `serde_json::json!`, schemars 1.2.2 `macros.rs:101-106`, so the
  constant is interpolated.)
- [ ] Run:
  ```powershell
  cargo fmt -p okf-jawn-contract
  cargo test --locked -p okf-jawn-contract -p okf-jawn-core -p okf-jawn-server -p xtask
  ```
  Expected: all `ok`, including `every_request_names_its_table_permission_first` (its
  synthesizer writes `"local"` for a pattern it does not recognise, which is a valid path).
- [ ] Commit `git add crates/contract/src/identity.rs crates/contract/tests/semantic.rs xtask/src/api.rs`:
  ```text
  fix(contract): publish the WorkspacePath rule a pattern can express.

  Why: the schema only said "1 to 4096 characters", so generated validators accepted absolute paths, empty segments, backslashes, drive letters and control characters that TryFrom rejects (design section 5, C: "WorkspacePath schema carries its full constraint").
  What changed: the WorkspacePath schema gains pattern ^[^/\\:\x00-\x1f]+(/[^/\\:\x00-\x1f]+)*$; the ".", ".." and ".git" segment rules stay in TryFrom. Tests: schema keywords in the contract, accept/reject cases through the generated create_item schema in xtask (the contract crate has no JSON Schema validator).
  Verified: cargo test --locked -p okf-jawn-contract --test semantic and cargo test --locked -p xtask failed before ("left: None"; "\"/client\" must be rejected") and pass after; cargo test --locked -p okf-jawn-contract -p okf-jawn-core -p okf-jawn-server -p xtask passes.
  Next: regenerate and commit generated outputs.

  Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>
  Claude-Session: https://claude.ai/code/session_01LoZr4ME3crLYjcTNn5GcAC
  ```

---

### Task C.11: Lint gate, regenerate, follow renamed generated names

**Files:**
- Regenerated (by `gen` only): `api/**`, `generated/cli/**`, `ui/src/api/generated/**`
- Modify: `ui/src/features/history/Changes.tsx:3`, `:6`; `ui/src/mcp-apps/main.tsx:8`, `:24`

**Interfaces:** Consumes everything above. Produces the generated outputs wave 2 branches from:
`api/operations.json` (69 entries, each with `operator_alias` and `destructive`),
`api/mcp-tools.json` (13 tools, the first named `workspaces`), no `api/schemars-splits.json`,
Zod exports `zDiffResponse`, `zFileChange`, `zGetSourcesResponse` (no `Input`/`Output`
suffixes), `zJobKind`, `zDraftConflictItem`.

- [ ] Lint gate (no reading found a contract lint violation other than the formatting cured in
  C.1; this step proves it):
  ```powershell
  cargo fmt -p okf-jawn-contract --check
  cargo fmt --all --check 2>&1 | Select-String '^Diff in' | ForEach-Object { ($_.Line -replace ':\d+:$','') } | Sort-Object -Unique | Set-Content .artifacts\cure-contract\fmt-after.txt
  Compare-Object (Get-Content .artifacts\cure-contract\fmt-before.txt) (Get-Content .artifacts\cure-contract\fmt-after.txt)
  cargo clippy --locked -p okf-jawn-contract -p okf-jawn-core -p okf-jawn-server -p okf-jawn-cli -p okf-jawn-mcp -p xtask --all-targets --keep-going -- -D warnings 2>&1 | Tee-Object .artifacts\cure-contract\clippy-after.txt | Out-Null
  function Summarize($file) { Select-String -Path $file -Pattern 'index\.html#(\w+)' | ForEach-Object { $_.Matches[0].Groups[1].Value } | Group-Object | Sort-Object Name | ForEach-Object { '{0,3} {1}' -f $_.Count, $_.Name } }
  Summarize .artifacts\cure-contract\clippy-before.txt
  Summarize .artifacts\cure-contract\clippy-after.txt
  Select-String -Path .artifacts\cure-contract\clippy-after.txt -Pattern 'result_large_err' | Measure-Object
  Select-String -Path .artifacts\cure-contract\clippy-after.txt -Pattern '^\s*--> crates[\\/](contract|cli)[\\/]'
  ```
  Expected:
  - `cargo fmt -p okf-jawn-contract --check`: exit 0.
  - `Compare-Object`: exactly two lines, both `<=`: `crates\contract\src\scope.rs` and
    `crates\contract\tests\scope.rs` (the other nine files are still unformatted and are not C's).
  - `result_large_err`: `Count : 0`, in every crate.
  - No error location under `crates\contract` or `crates\cli`.
  - `okf-jawn-core` (lib) still fails with exactly 13 errors, all present in the baseline:

    | Lint | File (line at `b205c4a`) | Cured by |
    | --- | --- | --- |
    | `arbitrary_source_item_ordering` | `crates/core/src/access.rs:113` | E |
    | `arbitrary_source_item_ordering` | `crates/core/src/dispatch.rs:35` | E |
    | `arbitrary_source_item_ordering` | `crates/core/src/storage.rs:77` | F |
    | `elidable_lifetime_names` | `crates/core/src/credentials.rs:41` | F |
    | `elidable_lifetime_names` | `crates/core/src/mutations.rs:81`, `:88`, `:95`, `:107` | E |
    | `needless_pass_by_value` | `crates/core/src/dispatch.rs:168` | E |
    | `doc_markdown` | `crates/core/src/mutations.rs:39`, `:103` (two) | E |
    | `format_collect` | `crates/core/src/mutations.rs:126` | E |

    (`dispatch.rs` lines are 1 to 2 higher after C's edits.) Because the core library does not pass,
    Clippy does not reach core's tests, `okf-jawn-server` or `okf-jawn-mcp`.
  - `xtask`: the same lints, in the same count, as `clippy-before.txt` (package D cures them).
  - Any lint that is in `clippy-after.txt` but not in `clippy-before.txt` is C's defect: fix it
    in `crates/contract/**` (or in the line C edited elsewhere), re-run, and commit it as
    `fix(contract): <lint> in <file>.` before regenerating.
- [ ] Regenerate (PowerShell, because `gen` runs cargo):
  ```powershell
  bun scripts/dev.mjs gen
  git status --porcelain
  ```
  Expected: changes only under `api/`, `generated/cli/` and `ui/src/api/generated/`;
  `api/schemars-splits.json` is deleted. Confirm:
  ```powershell
  bun -e "const o=JSON.parse(require('fs').readFileSync('api/operations.json','utf8')); console.log(o.length, o.filter(x=>x.visibility==='model').length, o.filter(x=>x.visibility==='app').length, o.filter(x=>x.destructive).length, o.every(x=>'operator_alias' in x))"
  bun -e "const t=JSON.parse(require('fs').readFileSync('api/mcp-tools.json','utf8')).tools; console.log(t.length, t[0].name)"
  Test-Path api\schemars-splits.json
  Select-String -Path ui\src\api\generated\zod.gen.ts -Pattern 'ResponseInput|ResponseOutput|FileChangeInput|FileChangeOutput' | Measure-Object
  Select-String -Path ui\src\api\generated\zod.gen.ts -Pattern 'export const (zDiffResponse|zFileChange|zJobKind|zDraftConflictItem|zWorkspacePath) '
  Select-String -Path ui\src\api\generated\types.gen.ts -Pattern 'expected_head' | Measure-Object
  ```
  Expected: `69 12 1 7 true`; `13 workspaces`; `False`; `Count : 0`; five export lines, the
  `zWorkspacePath` one ending `.regex(/^[^\/\\:\x00-\x1f]+(\/[^\/\\:\x00-\x1f]+)*$/);`;
  `Count : 0`.
- [ ] Run: `bun --bun run --cwd ui typecheck`
  Expected failure, in exactly two files: `src/features/history/Changes.tsx` line 3 and
  `src/mcp-apps/main.tsx` line 8, each `error TS2305` or `TS2724`: `Module
  '"…/api/generated/zod.gen"' has no exported member … 'zDiffResponseOutput'`. An error in any
  other authored file is a call site this plan did not find: fix it the same way (use the
  unsuffixed name) and name the file in the commit message.
- [ ] Implement:
  - `ui/src/features/history/Changes.tsx:3` →
    `import type { zDiffResponse } from '../../api/generated/zod.gen';`
  - `ui/src/features/history/Changes.tsx:6` → `  result: z.infer<typeof zDiffResponse>;`
  - `ui/src/mcp-apps/main.tsx:8` → `  zDiffResponse,`
  - `ui/src/mcp-apps/main.tsx:24` → `  zDiffResponse,`
  No other authored file names a split type, `DraftConflict` or `expected_head`
  (`ui/src/features/documents/Sources.tsx:16` reads `result.appearance &&`, which is valid for
  an optional field). `zGetSourcesResponse` is now the component schema itself; Hey API gives
  the per-operation alias a numeric suffix, as it already does for `GetAttentionResponse2`.
- [ ] Run:
  ```powershell
  bun --bun run --cwd ui typecheck
  bun --bun run --cwd ui test
  bun --bun run --cwd ui lint
  bun scripts/dev.mjs gen-check
  cargo test --locked -p okf-jawn-contract -p okf-jawn-core -p okf-jawn-server -p xtask
  ```
  Expected: typecheck exits 0; Vitest `Test Files 8 passed`; `lint` reports no error in
  `Changes.tsx` or `main.tsx` (two formatting errors in `tests/unit/layout.test.tsx` and
  `tests/unit/mcp-apps-dispatch.test.tsx` exist at base and are not C's); `gen-check` exits 0;
  cargo tests all `ok` (the contract test `every_mutation_example_carries_an_idempotency_key`
  reads the regenerated `api/examples`).
- [ ] Inspect `git diff --stat generated/cli` and read the `workspaces` entries in
  `generated/cli/_okf-jawn` and `generated/cli/okf-jawn.fish`: the new description contains
  braces and double quotes; it must appear as one quoted string per shell.
- [ ] Commit:
  ```powershell
  git add -A api generated/cli ui/src/api/generated
  git add ui/src/features/history/Changes.tsx ui/src/mcp-apps/main.tsx
  ```
  ```text
  chore(gen): regenerate from the cured contract and follow the unsplit Zod names.

  Why: wave 2 branches from this commit and must see the 13-field table, one schema per type and the new wire types (AGENTS.md generator-input rule).
  What changed: api/, generated/cli/ and ui/src/api/generated/ are the output of bun scripts/dev.mjs gen: 69 operations with operator_alias and destructive, 13 MCP tools including workspaces, no schemars-splits.json, DraftConflictItem, JobKind, WorkspacePath pattern, CommitRequest without expected_head. Changes.tsx and mcp-apps/main.tsx import zDiffResponse (was zDiffResponseOutput).
  Verified: bun scripts/dev.mjs gen-check exits 0; bun --bun run --cwd ui typecheck exits 0 (failed in the two files before the rename); bun --bun run --cwd ui test passes; cargo test --locked -p okf-jawn-contract -p okf-jawn-core -p okf-jawn-server -p xtask passes; clippy has 0 result_large_err.
  Next: orchestrator merges cure/contract; packages D, E, F, G branch from the result. Blocked for check-offline only: tests/foundation/policy.test.mjs still parses the 11-field table (package A).

  Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>
  Claude-Session: https://claude.ai/code/session_01LoZr4ME3crLYjcTNn5GcAC
  ```

---

### Package C acceptance

Run in `D:\okf\cure\contract` from PowerShell, tree clean at the tip of `cure/contract`.

| # | Command | Expected |
| --- | --- | --- |
| 1 | `git status --porcelain` | no output |
| 2 | `git diff --name-only 678f919..HEAD` | only paths in "Files allowed"; no `Cargo.toml`, `Cargo.lock`, `scripts/`, `tests/foundation/`, `qualification/` |
| 3 | `cargo fmt -p okf-jawn-contract --check` | exit 0 |
| 4 | `cargo fmt --all --check` | fails, naming exactly these 9 files and no other: `crates\core\src\{dispatch,drafts,mutations}.rs`, `crates\core\tests\dispatch.rs`, `crates\core\tests\support\{counting,mod}.rs`, `crates\server\tests\bindings.rs`, `qualification\docling\src\main.rs`, `xtask\src\api.rs` |
| 5 | `cargo clippy --locked -p okf-jawn-contract -p okf-jawn-cli --all-targets -- -D warnings` | exit 0. (CI never linted these targets; if the base commit already fails here in `crates/cli/src/main.rs`, which C does not touch, that error is reported to the orchestrator, not fixed by C.) |
| 6 | `cargo clippy --locked -p okf-jawn-contract -p okf-jawn-core -p okf-jawn-server -p okf-jawn-cli -p okf-jawn-mcp -p xtask --all-targets --keep-going -- -D warnings` | fails; zero `result_large_err`; `okf-jawn-core` (lib) has exactly the 13 errors in the table of Task C.11; nothing under `crates\contract` or `crates\cli`; `xtask` unchanged from the base commit |
| 7 | `cargo test --locked -p okf-jawn-contract -p okf-jawn-core -p okf-jawn-server -p xtask` | every `test result:` line `ok` |
| 8 | `cargo test --locked -p okf-jawn-contract --test schema` | `2 passed` |
| 9 | `bun scripts/dev.mjs gen-check` | exit 0 (committed outputs equal generator output) |
| 10 | `bun --bun run --cwd ui typecheck` | exit 0 |
| 11 | `bun --bun run --cwd ui test` | all test files pass |
| 12 | `Test-Path api\schemars-splits.json; Test-Path crates\contract\src\labels.rs` | `False`, `False` |
| 13 | Rule removal: in `crates/contract/src/history.rs` delete the `#[serde(default, skip_serializing_if = "Option::is_none")]` line above `old_path`, run #8, then `git checkout -- crates/contract/src/history.rs` | #8 fails; the message lists `(FileChange)` for every response that contains it (the job-returning operations through `ApiError`, and `diff_items response (FileChange)`) |
| 14 | Rule removal: in `crates/contract/src/error.rs` change `Option<Box<ErrorDetail>>` back to `Option<ErrorDetail>` and `Some(Box::new(detail))` to `Some(detail)`, run `cargo test --locked -p okf-jawn-contract --test error`, then `git checkout -- crates/contract/src/error.rs` | compile failure (`E0080` size assertion and `as_deref` bound) |

Not part of this gate, and red on this branch until package A merges: `bun scripts/dev.mjs
check-offline` and therefore `bun scripts/dev.mjs foundation` (see Deviations 4).

### Deviations

1. **Requirement 9, validator-backed cases are in `xtask`, not in `semantic.rs`.**
   `crates/contract/Cargo.toml:12-16` has no `jsonschema` dependency (`Cargo.lock:3207-3214`),
   and C may not touch manifests or the lockfile. `semantic.rs` asserts the schema keywords and
   the `TryFrom`-only rules; the accept/reject cases run through the generated
   `create_item.input.json` in `xtask/src/api.rs` `mod tests`, where `jsonschema` already is a
   dependency. Package D must keep that test. Alternative for the orchestrator: add
   `jsonschema.workspace = true` under `[dev-dependencies]` of the contract crate (a manifest
   and a one-line lock change) and move the test.
2. **One more tuple consumer than the shared list names.**
   `crates/core/tests/support/counting.rs:13-14` (package E's directory) matches the tuple and
   must change in the same commit, or `cargo test -p okf-jawn-core` stops compiling. C edits
   only its two pattern lines. `crates/contract/tests/scope.rs:19-20` also matches it (C's own).
3. **No shared test helpers yet.** `tests/support/check.rs` is produced by package E in wave
   2. C's tests obey the same rules with `std` only (`Result<(), Box<dyn Error>>`, `?`,
   `let … else { return Err(…) }`). Tests that have no fallible step are plain `fn`, so that
   `clippy::unnecessary_wraps` has nothing to flag.
4. **`check-offline` stays red on this branch.** `tests/foundation/policy.test.mjs:10` parses
   `operations.rs` with a regex for the 11-field row and asserts 66 operations and 11 model
   tools (`:14`, `:26`); after C it matches zero rows. That file is package A's, which rewrites
   it to read `api/operations.json`. `REPOSITORY-TREE.txt:216` still lists `labels.rs` (A's).
5. **SPEC.md §8 contradicts the owner's Snapshot decision.** `SPEC.md:91` says "if the head
   moved under a draft, the whole commit is rejected with a typed conflict (item, draft base,
   current revision, diff)". The contract now says per-item bases, unrelated head movement never
   blocks, every conflicting item listed. `SPEC.md` is in no package's file list; the
   integration owner must change that sentence.
6. **The stated Clippy and fmt gates cannot exit 0 after C.** Clippy: 13 pre-existing errors
   remain in `okf-jawn-core` (table in C.11; E and F), plus whatever `xtask` shows at base (D).
   By reading, `xtask/src/api.rs` at base has at least `too_many_lines` (`generate`, `:53-193`),
   `zero_sized_map_values` (`:238`), `arbitrary_source_item_ordering` (`struct Flattened`,
   `:348`, after functions) and `arithmetic_side_effects` (`checked += 1`, `:620`); CI never
   linted that crate, so C.0 records the real list. `cargo fmt --all --check`: nine files outside
   the contract crate remain (acceptance row 4).
7. **Destructive hint: seven operations set; candidates left `false` for the owner to rule
   on.** Set: `archive_workspace`, `restore_workspace`, `discard_draft`, `delete_item`,
   `restore_items`, `apply_names`, `revoke_connector`. Also overwrite or end something, left
   `false`: `cancel_job` (ends running work; uploads and recorded state are retained),
   `decline_proposal` (terminal status; proposal content retained), `save_draft` (replaces the
   caller's own previous autosave), `commit_items` (removes the snapshotted drafts after
   committing their content), `update_workspace`, `set_type`, `set_rules`, `move_item`,
   `set_lifecycle`, `correct_digest` (each overwrites versioned content; the prior state stays
   in history). None is agent-visible, so no MCP annotation changes either way.
8. **`destructiveHint` in the outputs C commits still comes from the old name list.**
   `xtask/src/api.rs:450-453` is package D's to replace. Until D lands,
   `x-tool-annotations.destructiveHint` is `false` for `discard_draft`, `restore_workspace` and
   `revoke_connector` while `api/operations.json` says `destructive: true`.
9. **`NotImplemented` → 501 has no server test and no OpenAPI response yet.** C adds the match
   arm in `crates/server/src/lib.rs` only because the match is exhaustive. The server test is
   package H's; the `501` entry in the OpenAPI error responses is in package D.
10. **`maxLength: 4096` counts characters; `TryFrom` counts bytes** (`identity.rs:233`). This
    existed before C and is not changed: for non-ASCII paths the schema is laxer than the type.
11. **`DraftConflict.items` "at least one" is a schema constraint only**
    (`#[schemars(length(min = 1))]`); serde will decode an empty list.
12. **First description with JSON punctuation.** The `list_workspaces` description contains
    `{"kind": "revision", "revision": head}`. It flows into clap help, four shell completions
    and a man page; C.11 has a step to read the generated CLI files.
13. **Not compiled while planning.** The planner could not run cargo, so no Rust snippet here
    has been compiled or linted; they were written against the repository sources and the
    vendored crate sources cited above. Executed in a scratch copy outside the repository: the
    table script (69 rows rewritten, 7 destructive) and Hey API's emission of the path pattern
    (type-checked; accepts and rejects the listed cases). The regeneration and the two-file UI
    rename were not executed; the call sites were found by search.
