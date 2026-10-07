## Package D: Generator

- **Branch:** `cure/generator`, created from `integration/foundation-cure` after package C is
  merged (base = that merge commit, called `<C>` below).
- **Worktree:** `D:\okf\cure\generator` (builds into its own `target\`). Scratch generator
  output goes to `D:\okf\cure\generator-out\` (outside every worktree).
- **Wave:** 2, in parallel with E (`crates/core/src/{dispatch,mutations,context,access}.rs`,
  core tests, `tests/support`), F (other core port files), G (`crates/mcp`).
- **Files allowed (exact):**
  - `xtask/src/api.rs`, `xtask/Cargo.toml`
  - `Cargo.toml` (delete the `utoipa` and `utoipa-axum` lines only) and `Cargo.lock` (removal
    of the packages that deletion orphans, nothing added), as instructed for this package
  - `ui/scripts/bundle-app.mjs`, `ui/scripts/catalog-entry.ts`; new
    `ui/scripts/app-declaration.ts`, `ui/scripts/catalog-response.ts`
  - `ui/openapi-ts.config.ts`, `scripts/lib/generation.mjs`
  - new `ui/src/features/views/spec-schema.ts`; `ui/src/features/views/Layout.tsx` (only to
    import it)
  - new `ui/tests/unit/app-declaration.test.ts`, `ui/tests/unit/catalog-response.test.ts`
- **Must not touch:** `crates/**`, `tests/**`, `qualification/**`, `scripts/dev.mjs`,
  `ui/src/features/views/catalog.ts` (no change is needed), any other `ui/src/**` or
  `ui/tests/**` file, `ui/package.json`, `bun.lock`, `ui/tsconfig*.json`, `vendors.json`,
  `README.md`. **Never stage `api/**`, `generated/cli/**` or `ui/src/api/generated/**`:** the
  orchestrator regenerates at merge. Not in this package: the generated "not implemented"
  application skeleton and the unbuilt-operations report.
- **SPEC/AGENTS sentences served:**
  - SPEC §14: "Rust identities and descriptions are authoritative for wire semantics. The
    emitted OpenAPI, JSON Schemas, tool catalog, CLI metadata and vendor-generated client
    distribute them."
  - SPEC §9: "Generate MCP schemas and annotations from the declared operation semantics."
  - SPEC §13: "repeat generation with no file-set or byte drift … semantic schema controls".
  - SPEC §10: "Renderer specs and reference bindings require runtime validation in addition
    to wire-schema validation."
  - `xtask/AGENTS.md`: "Keep generation independent, deterministic, and failure-transparent …
    Schema tests compare semantics; snapshots only identify drift."
  - Design §5 D and §8: "If a type legitimately differs between input and output, the contract
    package gives it two named types rather than relaxing the check."

**What C delivered that D relies on** (verify in D.0): the 13-field tuple;
`OperationInfo.operator_alias` and `.destructive`; `ErrorCode::NotImplemented`; one schema per
wire type (`crates/contract/tests/schema.rs`); `api/` without `schemars-splits.json`; in
`xtask/src/api.rs` the macro pattern, the `OperationInfo` literal, the `"x-cli-alias"`
expression and one extra test,
`workspace_path_schema_accepts_and_rejects_through_the_generated_schema`, which D keeps.

All cargo commands run from PowerShell in `D:\okf\cure\generator`. Commit procedure for every
commit: `git add` the listed paths, save the message exactly as shown to
`.artifacts\cure-generator\msg.txt` (UTF-8, no BOM), `git commit -F .artifacts\cure-generator\msg.txt`.
Tests follow the shared rules; `tests/support/check.rs` is being written by package E in
parallel, so D's Rust tests define `type TestResult` locally and use only `std`.

Line numbers for `xtask/src/api.rs` are those of `b205c4a`; C's edits moved everything below
line 22 down by two lines. Edits below are anchored by quoted code.

---

### Task D.0: Set up; confirm the base generates what is committed (no commit)

**Files:** Create (untracked) `.artifacts\cure-generator\*`, `D:\okf\cure\generator-out\*`.

**Interfaces:** Produces the helper `Compare-Api` used by later verification steps.

- [ ] Create the worktree and install:
  ```powershell
  git -C C:\Users\eayou\code_dir\okf-jawn worktree add -b cure/generator D:\okf\cure\generator integration/foundation-cure
  Set-Location D:\okf\cure\generator
  bun scripts/dev.mjs bootstrap
  git status --porcelain
  New-Item -ItemType Directory -Force .artifacts\cure-generator | Out-Null
  ```
  Expected: bootstrap completes; `git status --porcelain` prints nothing (the committed
  outputs equal generator output at `<C>`).
- [ ] Confirm C's deliverables:
  ```powershell
  Select-String -Path crates\contract\src\metadata.rs -Pattern 'pub operator_alias|pub destructive'
  Test-Path api\schemars-splits.json
  cargo test --locked -p okf-jawn-contract --test schema
  cargo test --locked -p xtask
  ```
  Expected: two matching lines; `False`; `2 passed`; xtask `2 passed`. If any differs, stop
  and report: D's base is wrong.
- [ ] Define the comparison helper (re-run this definition in every new PowerShell session):
  ```powershell
  function Compare-Api($scratch) {
    foreach ($name in 'openapi.json','openapi.yaml','operations.json','transports.json','mcp-tools.json','mcp-apps.json','schemas','forms','examples') {
      git diff --no-index --stat -- "api/$name" "$scratch/api/$name"
    }
  }
  $out = 'D:\okf\cure\generator-out'
  Remove-Item -Recurse -Force $out -ErrorAction SilentlyContinue
  cargo run --locked --package xtask -- generate --out "$out\base"
  Compare-Api "$out\base"
  ```
  Expected: `Compare-Api` prints nothing (the Rust generator reproduces the committed files).
- [ ] Record the lint baseline of the crate D must leave clean:
  ```powershell
  cargo clippy --locked -p xtask --all-targets -- -D warnings 2>&1 | Tee-Object .artifacts\cure-generator\clippy-before.txt | Select-String '^error'
  cargo fmt -p xtask --check 2>&1 | Select-String '^Diff in' | Measure-Object
  ```
  Expected: failures (by reading: `too_many_lines` on `generate`, `zero_sized_map_values` on
  `def_names`, `arbitrary_source_item_ordering` on `struct Flattened`, `arithmetic_side_effects`
  on `checked += 1`; CI never linted this crate, so the file is the record) and 7 or more fmt
  hunks in `xtask\src\api.rs`. D ends with both commands clean.

---

### Task D.1: Fail generation on a type with two wire shapes; use the names schemars returns

**Files:** Modify `xtask/src/api.rs`: header `:1-5`, imports `:7-18`, macro `:20-33`, types
`:35-51`, `generate` `:53-69` and four call sites, `typed` `:195-211`, delete `:213-407`
(`register_type` … `schema`) except as replaced below, `path_operation` `:424-446`,
`tool_definition` `:458-475`, `transport_operation` `:497-531`, `transport_response_schema`
`:533-541`, `mod tests` `:549-628`.

**Interfaces:**
- Consumes: `okf_jawn_contract::metadata::{OperationInfo, operations}`,
  `okf_jawn_contract::for_each_operation!` (13-field tuple); schemars 1.2.2
  `SchemaSettings::draft2020_12()`, `.for_serialize()`, `.for_deserialize()`,
  `.into_generator()`, `SchemaGenerator::into_root_schema_for::<T>()` (`generate.rs:108-187`,
  `:480-499`), `JsonSchema::schema_name()`.
- Produces (private to `xtask`):
  ```rust
  struct Registered { name: String, document: Value, components: BTreeMap<String, Value> }
  struct SharedTypes { api_error: Registered, upload: Registered, resource_metadata: Registered, health: Registered }
  struct TypedOperation { info: OperationInfo, request: Registered, response: Registered }
  fn register<T: JsonSchema>() -> Result<Registered, Box<dyn Error>>;
  const COMPONENT_PREFIX: &str = "#/components/schemas/";
  const DEFINITION_PREFIX: &str = "#/$defs/";
  ```
- Error text when a type has two shapes:
  `<Type>[, <Type>…]: serialize and deserialize schemas differ (reached through <Root>); give
  the type one wire shape or two named types`.
- Output files: byte-identical to `<C>` (no type has two shapes after package C).

- [ ] Write the failing tests. Replace the head of `mod tests` (the three `use` lines) with
  the block below and keep the two existing test functions under it unchanged for now:
  ```rust
  #[cfg(test)]
  mod tests {
      use std::error::Error;
      use std::path::{Path, PathBuf};

      use okf_jawn_contract::metadata::operations;
      use schemars::JsonSchema;
      use serde::{Deserialize, Serialize};
      use serde_json::{Value, json};

      type TestResult = Result<(), Box<dyn Error>>;

      /// Optional when read, required when written: the shape generation must refuse.
      #[derive(Serialize, Deserialize, JsonSchema)]
      struct Asymmetric {
          note: Option<String>,
      }

      /// One shape of its own, but it contains a type with two.
      #[derive(Serialize, Deserialize, JsonSchema)]
      struct Holder {
          inner: Asymmetric,
      }

      fn read(path: &Path) -> Result<Value, Box<dyn Error>> {
          Ok(serde_json::from_slice(&std::fs::read(path)?)?)
      }

      /// Every `$ref` target in `value`, in document order.
      fn references(value: &Value, found: &mut Vec<String>) {
          match value {
              Value::Object(map) => {
                  for (key, child) in map {
                      if key == "$ref"
                          && let Some(reference) = child.as_str()
                      {
                          found.push(reference.to_owned());
                      } else {
                          references(child, found);
                      }
                  }
              }
              Value::Array(items) => {
                  for item in items {
                      references(item, found);
                  }
              }
              Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_) => {}
          }
      }

      #[test]
      fn a_type_with_two_wire_shapes_is_refused_by_name() -> TestResult {
          let Err(direct) = super::register::<Asymmetric>() else {
              return Err("a type with two wire shapes must not register".into());
          };
          let direct = direct.to_string();
          assert!(
              direct.starts_with("Asymmetric: serialize and deserialize schemas differ"),
              "{direct}"
          );
          let Err(nested) = super::register::<Holder>() else {
              return Err("a type containing a two-shape type must not register".into());
          };
          let nested = nested.to_string();
          assert!(
              nested.starts_with("Asymmetric: serialize and deserialize schemas differ"),
              "{nested}"
          );
          assert!(nested.contains("reached through Holder"), "{nested}");
          Ok(())
      }

      #[test]
      fn every_operation_schema_compiles_and_every_reference_resolves() -> TestResult {
          let directory = tempfile::tempdir()?;
          super::generate(directory.path())?;
          assert!(
              !directory.path().join("schemars-splits.json").exists(),
              "no split receipt is written"
          );
          let api = read(&directory.path().join("openapi.json"))?;
          let components = api
              .pointer("/components/schemas")
              .and_then(Value::as_object)
              .ok_or("components.schemas")?;
          let mut published = Vec::new();
          references(&api, &mut published);
          assert!(
              published.len() > operations().len(),
              "the document must reference its components"
          );
          for reference in &published {
              let name = reference
                  .strip_prefix(super::COMPONENT_PREFIX)
                  .ok_or_else(|| format!("{reference} is not a component reference"))?;
              assert!(components.contains_key(name), "{reference} names no component");
          }
          for operation in operations() {
              for side in ["input", "output"] {
                  let label = format!("{}.{side}", operation.id);
                  let schema = read(
                      &directory
                          .path()
                          .join("schemas")
                          .join(format!("{label}.json")),
                  )?;
                  let definitions = schema.get("$defs").and_then(Value::as_object);
                  let mut local = Vec::new();
                  references(&schema, &mut local);
                  for reference in &local {
                      let name = reference
                          .strip_prefix(super::DEFINITION_PREFIX)
                          .ok_or_else(|| format!("{label}: {reference} is not a local definition"))?;
                      assert!(
                          definitions.is_some_and(|definitions| definitions.contains_key(name)),
                          "{label}: {reference} names no definition"
                      );
                  }
                  let validator = jsonschema::validator_for(&schema)
                      .map_err(|error| format!("{label}: {error}"))?;
                  assert!(
                      !validator.is_valid(&json!(7)),
                      "{label}: every request and response is an object"
                  );
              }
          }
          Ok(())
      }
  ```
- [ ] Run: `cargo test --locked -p xtask`
  Expected failure: `error[E0425]: cannot find function `register` in module `super`` and
  `cannot find value `COMPONENT_PREFIX` in module `super``.
- [ ] Implement. Replace the header and imports (`:1-18`) with:
  ```rust
  //! Deterministic OpenAPI and JSON Schema output from the complete Rust operation surface.
  //!
  //! Every schema is the schemars (draft 2020-12) schema of a contract type, published under the
  //! name schemars gives it. A type has one wire shape: generation fails, naming the type, when
  //! its serialize and deserialize schemas differ.

  use std::collections::BTreeMap;
  use std::error::Error;
  use std::path::Path;

  use okf_jawn_contract::access::{Permission, ResourceMetadata};
  use okf_jawn_contract::error::ApiError;
  use okf_jawn_contract::health::HealthResponse;
  use okf_jawn_contract::import::Upload;
  use okf_jawn_contract::metadata::{OperationInfo, operations};
  use okf_jawn_contract::transport::{TransportAuth, TransportOperation};
  use schemars::{JsonSchema, generate::SchemaSettings};
  use serde_json::{Map, Value, json};
  use utoipa::openapi::OpenApiBuilder;
  use utoipa::openapi::info::InfoBuilder;

  use crate::output::write_json;
  ```
- [ ] Replace the macro, `TypedOperation` and `SplitLog` (`:20-51`) with:
  ```rust
  macro_rules! generate_operations {
      ($(($id:ident, $request:ty, $response:ty, $path:literal, $label:literal, $alias:literal,
          $operator:literal, $visibility:literal, $permission:ident, $ui:literal, $status:literal,
          $destructive:literal, $description:literal)),* $(,)?) => {
          /// Pair every row of `operations()` with the schemas of its request and response types.
          fn typed_operations() -> Result<Vec<TypedOperation>, Box<dyn Error>> {
              operations()
                  .into_iter()
                  .map(|info| {
                      let id = info.id;
                      match id {
                          $(stringify!($id) => typed::<$request, $response>(info),)*
                          other => Err(format!("operation {other} has no declared types").into()),
                      }
                  })
                  .collect()
          }
      };
  }

  /// One declared operation with the schemas of its request and response types.
  struct TypedOperation {
      info: OperationInfo,
      request: Registered,
      response: Registered,
  }

  /// One contract type: its component name, its standalone schema document, and every named
  /// component (itself and its definitions) it contributes to the OpenAPI document.
  struct Registered {
      name: String,
      document: Value,
      components: BTreeMap<String, Value>,
  }

  /// Types the document refers to outside the operation table.
  struct SharedTypes {
      api_error: Registered,
      upload: Registered,
      resource_metadata: Registered,
      health: Registered,
  }

  const COMPONENT_PREFIX: &str = "#/components/schemas/";
  const DEFINITION_PREFIX: &str = "#/$defs/";
  ```
  (The table itself now supplies `OperationInfo`; the literal that duplicated
  `metadata.rs` is gone. The generated `match` on `id` stays: it is the table expanded, the
  only way `macro_rules!` can pair a runtime row with its two types.)
- [ ] In `generate`, replace from `let typed = typed_operations()?;` through the end of the
  first `for operation in &typed { … }` loop (the one calling `insert_schema` and
  `record_split`) with:
  ```rust
      let typed = typed_operations()?;
      let shared = SharedTypes {
          api_error: register::<ApiError>()?,
          upload: register::<Upload>()?,
          resource_metadata: register::<ResourceMetadata>()?,
          health: register::<HealthResponse>()?,
      };
      let mut schemas = BTreeMap::new();
      let outside = [
          &shared.api_error,
          &shared.upload,
          &shared.resource_metadata,
          &shared.health,
      ];
      let declared = typed
          .iter()
          .flat_map(|operation| [&operation.request, &operation.response]);
      for registered in outside.into_iter().chain(declared) {
          for (name, schema) in &registered.components {
              insert_schema(&mut schemas, name.clone(), schema.clone())?;
          }
      }
  ```
  and in the rest of `generate`:
  - `json!({"post": path_operation(operation)})` → `json!({"post": path_operation(operation, &shared)})`
  - `&operation.input,` → `&operation.request.document,`
  - `&operation.output,` → `&operation.response.document,`
  - `transport_operation(&transport)` → `transport_operation(&transport, &shared)`
  - delete the whole `if !splits.pairs.is_empty() { … write_json(&directory.join("schemars-splits.json"), &receipt)?; }` block.
- [ ] Replace `typed` and everything from `register_type` through `schema` (`:195-407`:
  `typed`, `register_type`, `register_request_response`, `record_split`, `apply_renames`,
  `Flattened`, `flatten_root`, `rewrite_refs`, `schema`) with:
  ```rust
  fn typed<Q: JsonSchema, R: JsonSchema>(
      info: OperationInfo,
  ) -> Result<TypedOperation, Box<dyn Error>> {
      Ok(TypedOperation {
          info,
          request: register::<Q>()?,
          response: register::<R>()?,
      })
  }

  /// Register one type under the name schemars gives it.
  ///
  /// Fails when the type, or a type it contains, serializes with a different schema than it
  /// deserializes with.
  fn register<T: JsonSchema>() -> Result<Registered, Box<dyn Error>> {
      let name = T::schema_name().into_owned();
      let document = schema_document::<T>(false)?;
      let serialized = schema_document::<T>(true)?;
      if document != serialized {
          return Err(format!(
              "{}: serialize and deserialize schemas differ (reached through {name}); give the type one wire shape or two named types",
              differing_types(&name, &document, &serialized).join(", ")
          )
          .into());
      }
      let mut body = document
          .as_object()
          .cloned()
          .ok_or_else(|| format!("{name}: schemars root schema is not an object"))?;
      let definitions = body.remove("$defs");
      body.remove("$schema");
      body.remove("title");
      let mut components = BTreeMap::new();
      if let Some(Value::Object(definitions)) = definitions {
          for (definition, schema) in definitions {
              components.insert(definition, component_references(schema));
          }
      }
      insert_schema(
          &mut components,
          name.clone(),
          component_references(Value::Object(body)),
      )?;
      Ok(Registered {
          name,
          document,
          components,
      })
  }

  fn schema_document<T: JsonSchema>(serialize: bool) -> Result<Value, serde_json::Error> {
      let settings = SchemaSettings::draft2020_12();
      let settings = if serialize {
          settings.for_serialize()
      } else {
          settings.for_deserialize()
      };
      serde_json::to_value(settings.into_generator().into_root_schema_for::<T>())
  }

  /// Names of the definitions whose two schemas differ, and the root when its own body differs.
  fn differing_types(root: &str, deserialize: &Value, serialize: &Value) -> Vec<String> {
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

  fn without_definitions(schema: &Value) -> Value {
      let mut body = schema.clone();
      if let Some(object) = body.as_object_mut() {
          object.remove("$defs");
      }
      body
  }

  /// Point every local `$defs` reference at the OpenAPI component of the same name.
  fn component_references(value: Value) -> Value {
      match value {
          Value::Object(map) => {
              let mut out = Map::new();
              for (key, child) in map {
                  if key == "$ref"
                      && let Some(reference) = child.as_str()
                      && let Some(name) = reference.strip_prefix(DEFINITION_PREFIX)
                  {
                      out.insert(key, Value::String(format!("{COMPONENT_PREFIX}{name}")));
                  } else {
                      out.insert(key, component_references(child));
                  }
              }
              Value::Object(out)
          }
          Value::Array(items) => {
              Value::Array(items.into_iter().map(component_references).collect())
          }
          other => other,
      }
  }

  fn component_reference(name: &str) -> Value {
      json!({"$ref": format!("{COMPONENT_PREFIX}{name}")})
  }

  fn optional(value: &str) -> Option<&str> {
      (!value.is_empty()).then_some(value)
  }
  ```
  `insert_schema` (`:409-422`) stays as it is.
- [ ] Replace `path_operation` (`:424-446`) with (the literal `"#/components/schemas/…"`
  strings are gone; the status list and `annotations` are unchanged until D.4):
  ```rust
  fn path_operation(operation: &TypedOperation, shared: &SharedTypes) -> Value {
      let info = &operation.info;
      let mut responses = Map::new();
      responses.insert(
          info.success_status.to_string(),
          json!({
              "description": "Successful operation result",
              "content": {"application/json": {"schema": component_reference(&operation.response.name)}}
          }),
      );
      for status in [
          "400", "401", "403", "404", "409", "413", "422", "500", "503",
      ] {
          responses.insert(
              status.to_owned(),
              json!({
                  "description": "Structured application failure",
                  "content": {"application/json": {"schema": component_reference(&shared.api_error.name)}}
              }),
          );
      }
      json!({
          "operationId": info.id,
          "summary": info.label,
          "description": info.description,
          "tags": [info.path.split('/').nth(2).unwrap_or("operations")],
          "requestBody": {
              "required": true,
              "content": {"application/json": {"schema": component_reference(&operation.request.name)}}
          },
          "responses": responses,
          "x-agent-tool": info.visibility == "model",
          "x-mcp-tool": !info.alias.is_empty(),
          "x-tool-visibility": info.visibility,
          "x-agent-alias": info.alias,
          "x-operator-label": info.label,
          "x-cli-alias": optional(info.operator_alias),
          "x-permission": info.permission,
          "x-ui-resource": resource_uri(info.ui),
          "x-tool-annotations": annotations(info)
      })
  }
  ```
- [ ] In `tool_definition` (`:458-475`) replace its first statement with:
  ```rust
      let mut tool = json!({
          "name": operation.info.alias,
          "title": operation.info.label,
          "description": operation.info.description,
          "inputSchema": operation.request.document,
          "outputSchema": operation.response.document,
          "annotations": annotations(&operation.info)
      });
  ```
- [ ] `transport_operation` (`:497`): signature →
  `fn transport_operation(operation: &TransportOperation, shared: &SharedTypes) -> Value {`;
  inside it `transport_response_schema(operation.id, operation.response_media)` →
  `transport_response_schema(operation, shared)` and
  `okf_jawn_contract::transport::TransportAuth::Public` → `TransportAuth::Public`.
- [ ] Replace `transport_response_schema` (`:533-541`) with:
  ```rust
  /// The transport table has no response-type column; these three routes return contract types.
  fn transport_response_schema(operation: &TransportOperation, shared: &SharedTypes) -> Value {
      match operation.id {
          "upload_content" => component_reference(&shared.upload.name),
          "get_resource_metadata" => component_reference(&shared.resource_metadata.name),
          "liveness" => component_reference(&shared.health.name),
          _ if operation.response_media.contains("json") => any_value_schema(),
          _ => json!({"type": "string"}),
      }
  }
  ```
- [ ] Order the items for `clippy::arbitrary_source_item_ordering` (clippy.toml groups):
  `use`, `macro_rules!`, the three `struct`s, the two `const`s, then every `fn`, then
  `mod tests`. The `okf_jawn_contract::for_each_operation!(generate_operations);` line may stay
  between functions (its expansion is not linted).
- [ ] Run:
  ```powershell
  cargo fmt -p xtask
  cargo test --locked -p xtask
  cargo run --locked --package xtask -- generate --out "$out\d1"
  Compare-Api "$out\d1"
  ```
  Expected: `test result: ok. 4 passed`; `Compare-Api` prints nothing (output is unchanged).
- [ ] Rule-removal check (no commit): in `register`, change `if document != serialized {` to
  `if false {`; `cargo test --locked -p xtask` → `a_type_with_two_wire_shapes_is_refused_by_name`
  fails with `a type with two wire shapes must not register`. Restore the line.
- [ ] Commit `git add xtask/src/api.rs`:
  ```text
  fix(xtask): fail generation when a type has two wire shapes.

  Why: the generator silently renamed a type to <Name>Input/<Name>Output when its serialize and deserialize schemas differed, and hard-coded component names for ApiError, Upload, ResourceMetadata and HealthResponse (design section 5, D; section 8 "the contract package gives it two named types rather than relaxing the check").
  What changed: register::<T>() compares the two schemars contracts and returns an error naming the differing type; components and $ref targets use the names schemars returns; the Input/Output rename machinery, SplitLog and schemars-splits.json emission are deleted; typed_operations() takes OperationInfo from operations() instead of rebuilding it. Output files are byte-identical.
  Verified: cargo test --locked -p xtask failed with E0425 before, 4 passed after; generating into a scratch directory and git diff --no-index against api/ shows no difference; with the comparison disabled the new test fails.
  Next: assemble the OpenAPI document as plain JSON and drop utoipa.

  Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>
  Claude-Session: https://claude.ai/code/session_01LoZr4ME3crLYjcTNn5GcAC
  ```

---

### Task D.2: Assemble the OpenAPI document as plain JSON; remove `utoipa`

**Decision (by reading):** after D.1, `utoipa` is used in three places only:
`OpenApiBuilder`/`InfoBuilder` (`:70-80`), a `serde_json::from_value::<PathItem>` round trip
per path (`:82-116`), and `any_value_schema` (`:543-547`), which exists only because utoipa
cannot deserialize `{}`. Nothing else in the workspace uses it (`rg utoipa` finds only
`Cargo.toml:19-20`, `xtask/Cargo.toml:18`, `xtask/src/api.rs` and prose); `utoipa-axum` is
declared in `Cargo.toml:20` and used by no crate (it is not in `Cargo.lock`). So the document
is emitted as `serde_json` and both workspace entries go.

**Files:**
- Modify: `xtask/src/api.rs` (`generate`, new `openapi_document`, `write_forms`,
  `app_resources`; delete `any_value_schema`; imports)
- Modify: `xtask/Cargo.toml:18`, `Cargo.toml:19-20`, `Cargo.lock`

**Interfaces:**
- Produces: `fn openapi_document(typed: &[TypedOperation], shared: &SharedTypes) -> Result<Value, Box<dyn Error>>`
  returning `{"openapi":"3.1.0","info":{…},"paths":{…},"security":[…],"components":{"schemas":{…},"securitySchemes":{…}}}`.
- Output files: byte-identical to `<C>`.

- [ ] This is a refactor; its failing check is the comparison, not a new test. First remove
  the dependency so the build proves nothing else needs it:
  - `xtask/Cargo.toml`: delete line 18 (`utoipa.workspace = true`).
  - `Cargo.toml`: delete lines 19-20 (`utoipa = { … }` and `utoipa-axum = { … }`).
  Run: `cargo check --locked -p xtask` → expected failure: `error: the lock file … needs to
  be updated but --locked was passed`.
- [ ] Update the lockfile and prove the change only removes:
  ```powershell
  cargo update --workspace --offline
  git diff --numstat -- Cargo.lock
  git diff -U0 -- Cargo.lock | Select-String -Pattern '^\+(?!\+\+)'
  git diff -U0 -- Cargo.lock | Select-String -Pattern '^-name = '
  ```
  Expected: cargo prints `Removing utoipa v6.0.0` and `Removing utoipa-gen v6.0.1`;
  `--numstat` shows `0` added lines; the second command prints nothing (no added line); the
  third prints exactly `-name = "utoipa"` and `-name = "utoipa-gen"` (`Cargo.lock:5826-5848`
  at `b205c4a`; their dependencies `indexmap`, `serde`, `serde_json`, `yaml_serde`,
  `proc-macro2`, `quote`, `syn 3.0.6`, `uuid` all have other dependents). If `--offline`
  cannot resolve, run `cargo update --workspace` and apply the same three checks. If any line
  is added, or any other package is removed, stop and report with the diff.
  Run: `cargo check --locked -p xtask` → the expected failure is now in our code:
  `error[E0433]` / `error[E0432]` naming `utoipa` at the two `use utoipa::…` lines.
- [ ] Implement in `xtask/src/api.rs`:
  - delete the two `use utoipa::…;` lines;
  - delete `fn any_value_schema` and its comment; in `transport_operation` and
    `transport_response_schema` replace each `any_value_schema()` with `json!({})`;
  - add a constant after `DEFINITION_PREFIX`:
    ```rust
    const TRANSPORT_METHODS: [&str; 4] = ["get", "put", "post", "delete"];
    ```
  - replace `generate` entirely with:
    ```rust
    pub(crate) fn generate(directory: &Path) -> Result<(), Box<dyn Error>> {
        let typed = typed_operations()?;
        let shared = SharedTypes {
            api_error: register::<ApiError>()?,
            upload: register::<Upload>()?,
            resource_metadata: register::<ResourceMetadata>()?,
            health: register::<HealthResponse>()?,
        };
        let document = openapi_document(&typed, &shared)?;
        write_json(&directory.join("openapi.json"), &document)?;
        std::fs::write(
            directory.join("openapi.yaml"),
            yaml_serde::to_string(&document)?,
        )?;
        let schemas = directory.join("schemas");
        for operation in &typed {
            let id = operation.info.id;
            write_json(
                &schemas.join(format!("{id}.input.json")),
                &operation.request.document,
            )?;
            write_json(
                &schemas.join(format!("{id}.output.json")),
                &operation.response.document,
            )?;
        }
        write_json(
            &directory.join("operations.json"),
            &serde_json::to_value(operations())?,
        )?;
        write_json(
            &directory.join("transports.json"),
            &serde_json::to_value(okf_jawn_contract::transport::operations())?,
        )?;
        let tools: Vec<Value> = typed
            .iter()
            .filter(|operation| !operation.info.alias.is_empty())
            .map(tool_definition)
            .collect();
        write_json(&directory.join("mcp-tools.json"), &json!({"tools": tools}))?;
        write_json(&directory.join("mcp-apps.json"), &app_resources())?;
        write_forms(&directory.join("forms"))?;
        crate::fixtures::generate(&directory.join("examples"))?;
        Ok(())
    }

    /// Assemble the OpenAPI 3.1 document from the registered schemas and both route tables.
    fn openapi_document(
        typed: &[TypedOperation],
        shared: &SharedTypes,
    ) -> Result<Value, Box<dyn Error>> {
        let mut schemas = BTreeMap::new();
        let outside = [
            &shared.api_error,
            &shared.upload,
            &shared.resource_metadata,
            &shared.health,
        ];
        let declared = typed
            .iter()
            .flat_map(|operation| [&operation.request, &operation.response]);
        for registered in outside.into_iter().chain(declared) {
            for (name, schema) in &registered.components {
                insert_schema(&mut schemas, name.clone(), schema.clone())?;
            }
        }
        let mut paths: BTreeMap<String, Map<String, Value>> = BTreeMap::new();
        for operation in typed {
            let item = paths.entry(operation.info.path.to_owned()).or_default();
            if item
                .insert("post".to_owned(), path_operation(operation, shared))
                .is_some()
            {
                return Err(format!("{} is declared twice", operation.info.path).into());
            }
        }
        for transport in okf_jawn_contract::transport::operations() {
            if !TRANSPORT_METHODS.contains(&transport.method) {
                return Err(format!(
                    "transport {} declares unsupported method {}",
                    transport.id, transport.method
                )
                .into());
            }
            let item = paths.entry(transport.path.to_owned()).or_default();
            if item
                .insert(
                    transport.method.to_owned(),
                    transport_operation(&transport, shared),
                )
                .is_some()
            {
                return Err(format!(
                    "{} {} is declared twice",
                    transport.method, transport.path
                )
                .into());
            }
        }
        Ok(json!({
            "openapi": "3.1.0",
            "info": {
                "title": "okf-jawn",
                "version": env!("CARGO_PKG_VERSION"),
                "description": "Complete intended API. A declared route is not a claim of implemented application behavior."
            },
            "paths": paths,
            "security": [{"bearerAuth": []}, {"browserSession": []}],
            "components": {
                "schemas": schemas,
                "securitySchemes": {
                    "bearerAuth": {
                        "type": "http",
                        "scheme": "bearer",
                        "description": "Hosted: a WorkOS Connect access token. Local: an opaque connector secret issued by create_connector."
                    },
                    "browserSession": {
                        "type": "apiKey",
                        "in": "cookie",
                        "name": "okf-session",
                        "description": "Local-owner or WorkOS browser session; cookie-authenticated writes also require the CSRF token."
                    }
                }
            }
        }))
    }

    /// The MCP App resources this build declares.
    fn app_resources() -> Value {
        json!({
            "resources": [{
                "uri": "ui://okf-jawn/app.html",
                "mimeType": "text/html;profile=mcp-app",
                "csp": {"connectDomains": [], "resourceDomains": []}
            }]
        })
    }

    fn write_forms(directory: &Path) -> Result<(), Box<dyn Error>> {
        write_json(
            &directory.join("naming-rules.schema.json"),
            &form_schema::<okf_jawn_contract::conventions::NamingRules>()?,
        )?;
        write_json(
            &directory.join("view.schema.json"),
            &form_schema::<okf_jawn_contract::views::ViewDocument>()?,
        )?;
        write_json(
            &directory.join("type.schema.json"),
            &form_schema::<okf_jawn_contract::item::TypeDefinition>()?,
        )?;
        Ok(())
    }
    ```
  (`app_resources` keeps the old shape here so this commit changes no output; D.5 changes it.)
- [ ] Run:
  ```powershell
  cargo fmt -p xtask
  cargo test --locked -p xtask
  cargo run --locked --package xtask -- generate --out "$out\d2"
  Compare-Api "$out\d2"
  git grep -n utoipa -- xtask Cargo.toml Cargo.lock
  ```
  Expected: `4 passed`; `Compare-Api` prints nothing; `git grep` finds nothing. If `Compare-Api`
  shows a difference, the plain-JSON builder does not reproduce what the round trip through
  utoipa wrote: read the diff, make the builder emit the committed shape, and re-run. Do not
  accept a difference in this commit.
- [ ] Commit `git add xtask/src/api.rs xtask/Cargo.toml Cargo.toml Cargo.lock`:
  ```text
  refactor(xtask): assemble the OpenAPI document as plain JSON and remove utoipa.

  Why: utoipa was only an InfoBuilder and a PathItem serde round trip, and needed a {"type": null} workaround because it cannot read an empty schema; schemars is the schema authority (design section 5, D).
  What changed: openapi_document() builds the document with serde_json; duplicate paths and unsupported transport methods are errors; any_value_schema is gone. utoipa is removed from xtask/Cargo.toml and utoipa/utoipa-axum from the workspace dependencies; Cargo.lock loses utoipa and utoipa-gen and gains nothing.
  Verified: cargo test --locked -p xtask passes (4); scratch generation compared with git diff --no-index against api/ shows no difference; git diff --numstat Cargo.lock shows 0 added lines.
  Next: validate every example against the operation it names. Stale prose for the orchestrator: vendors.json (utoipa entry), README.md:75, .agents/skills/regenerate-api/SKILL.md:8.

  Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>
  Claude-Session: https://claude.ai/code/session_01LoZr4ME3crLYjcTNn5GcAC
  ```

---

### Task D.3: Every example validates against the operation it names

**How examples are matched** (`xtask/src/fixtures.rs`): the generator writes typed request
values as `examples/<name>.json` (`read-item.json`, `list-items.json`,
`create-workspace.json`). The rule, now enforced: the file stem with `-` replaced by `_` is an
operation id, and the file is a request for that operation. It is validated against
`schemas/<id>.input.json` and against the OpenAPI component that operation's `requestBody`
references. A file whose stem names no operation fails the test (the old test skipped it).

**Files:** Modify `xtask/src/api.rs` `mod tests` (replace
`request_schemas_agree_on_all_example_fixtures`, `:555-627`).

**Interfaces:** Consumes `super::generate`, `jsonschema::validator_for(&Value) ->
Result<Validator, ValidationError<'static>>` and `Validator::validate(&self, &Value) ->
Result<(), ValidationError<'_>>` (`jsonschema-0.58.5/src/lib.rs:1510`, `validator.rs`).

- [ ] Write the test. In `mod tests`: change `use std::path::{Path, PathBuf};` to
  `use std::path::Path;`, add `use std::collections::BTreeSet;` as the first import, delete
  `request_schemas_agree_on_all_example_fixtures`, and add:
  ```rust
      #[test]
      fn every_example_validates_against_the_operation_it_names() -> TestResult {
          let directory = tempfile::tempdir()?;
          super::generate(directory.path())?;
          let api = read(&directory.path().join("openapi.json"))?;
          let components = api.get("components").ok_or("components")?;
          let table = operations();
          let mut checked = BTreeSet::new();
          for entry in std::fs::read_dir(directory.path().join("examples"))? {
              let path = entry?.path();
              let file = path
                  .file_name()
                  .and_then(|name| name.to_str())
                  .ok_or("example file name")?
                  .to_owned();
              let stem = file
                  .strip_suffix(".json")
                  .ok_or_else(|| format!("{file}: examples are JSON files"))?;
              let id = stem.replace('-', "_");
              let operation = table
                  .iter()
                  .find(|operation| operation.id == id)
                  .ok_or_else(|| format!("{file} names no declared operation"))?;
              let example = read(&path)?;
              let input = read(
                  &directory
                      .path()
                      .join("schemas")
                      .join(format!("{id}.input.json")),
              )?;
              let pointer = format!(
                  "/paths/{}/post/requestBody/content/application~1json/schema/$ref",
                  operation.path.replace('/', "~1")
              );
              let reference = api
                  .pointer(&pointer)
                  .and_then(Value::as_str)
                  .ok_or_else(|| format!("{id}: no request component"))?;
              let published = json!({
                  "$schema": "https://json-schema.org/draft/2020-12/schema",
                  "$ref": reference,
                  "components": components
              });
              for (source, schema) in [
                  ("its input schema", &input),
                  ("its OpenAPI component", &published),
              ] {
                  let validator = jsonschema::validator_for(schema)
                      .map_err(|error| format!("{file}: {source}: {error}"))?;
                  if let Err(error) = validator.validate(&example) {
                      return Err(format!("{file} does not validate against {source}: {error}").into());
                  }
              }
              checked.insert(id);
          }
          assert!(
              checked.len() >= 3,
              "the generator writes at least its three typed examples, checked {checked:?}"
          );
          Ok(())
      }
  ```
- [ ] Show that it fails where the old test passed (no commit). In `xtask/src/fixtures.rs:29`
  change `"read-item.json"` to `"read-items.json"`; run `cargo test --locked -p xtask`.
  Expected: `every_example_validates_against_the_operation_it_names` fails with
  `read-items.json names no declared operation` (the deleted test skipped such a file and
  passed). Run `git checkout -- xtask/src/fixtures.rs`.
- [ ] Run:
  ```powershell
  cargo fmt -p xtask
  cargo test --locked -p xtask
  ```
  Expected: `4 passed` (two from D.1, this one, and C's WorkspacePath test).
- [ ] Commit `git add xtask/src/api.rs`:
  ```text
  test(xtask): validate every generated example against the operation it names.

  Why: the single agreement test skipped any example whose file name matched no operation and used unchecked arithmetic; with D.1's test it now covers every operation schema, every $ref and every example (design section 5, D).
  What changed: examples/<kebab-id>.json must name a declared operation and validate against schemas/<id>.input.json and that operation's OpenAPI request component; a misnamed example fails.
  Verified: cargo test --locked -p xtask passes (4); renaming read-item.json to read-items.json in fixtures.rs makes the new test fail with "names no declared operation".
  Next: read tool hints and the 501 response from the table.

  Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>
  Claude-Session: https://claude.ai/code/session_01LoZr4ME3crLYjcTNn5GcAC
  ```

---

### Task D.4: Hints, operator alias and the 501 response come from the table

**Files:** Modify `xtask/src/api.rs`: `annotations` (`:448-456`), the status list in
`path_operation`, constants, `mod tests`.

**Interfaces:**
- Consumes `OperationInfo.destructive: bool`, `OperationInfo.operator_alias: &'static str`.
- Produces: `x-tool-annotations.destructiveHint == info.destructive` and
  `x-cli-alias == operator_alias | null` for every operation; a `501` `ApiError` response on
  every operation.

**Every match on an operation or transport id in `xtask`, and what happens to it:**

| Where (`b205c4a`) | What | Outcome |
| --- | --- | --- |
| `api.rs:450-453` | `matches!(info.id, "delete_item" \| "restore_items" \| "apply_names" \| "archive_workspace")` | Deleted here; `info.destructive`. |
| `api.rs:443` | `labels::operator_alias(operation.info.id)` | Already replaced by C with `info.operator_alias`; D.1 wraps it in `optional`. |
| `api.rs:59-61`, `:434` | literal `#/components/schemas/{ApiError,Upload,ResourceMetadata,HealthResponse}` | Deleted in D.1; `Registered.name`. |
| `api.rs:533-541` | `match id { "upload_content" \| "get_resource_metadata" \| "liveness" }` | Stays (D.1 form): `TransportOperation` has no response-type column and the contract is not D's to change. Reported in Deviations 5. |
| `api.rs:110-116` | `match transport.method` | Deleted in D.2; the method is a JSON key, checked against `TRANSPORT_METHODS`. |
| `api.rs:25` (macro) | one arm per table row | Stays (D.1 form): it is the table, expanded. |
| `api.rs:575`, `:592` (test) | example file name → operation id | Replaced in D.3 by the stated rule. |
| `fixtures.rs:29`, `:44`, `:48` | example file names | Stay: they are the typed examples; D.3 checks each names an operation. |
| `api.rs:463`, `:477-484` | `visibility == "app"`, `ui` key empty or not | Stay: table columns, not ids. |

- [ ] Write the failing test; add to `mod tests`:
  ```rust
      #[test]
      fn hints_aliases_and_the_not_implemented_response_come_from_the_table() -> TestResult {
          let directory = tempfile::tempdir()?;
          super::generate(directory.path())?;
          let api = read(&directory.path().join("openapi.json"))?;
          let mut destructive = 0_usize;
          for operation in operations() {
              let pointer = format!("/paths/{}/post", operation.path.replace('/', "~1"));
              let post = api
                  .pointer(&pointer)
                  .ok_or_else(|| format!("{}: no path", operation.id))?;
              assert_eq!(
                  post.pointer("/x-tool-annotations/destructiveHint"),
                  Some(&json!(operation.destructive)),
                  "{} destructive hint",
                  operation.id
              );
              let alias = if operation.operator_alias.is_empty() {
                  Value::Null
              } else {
                  json!(operation.operator_alias)
              };
              assert_eq!(
                  post.get("x-cli-alias"),
                  Some(&alias),
                  "{} operator alias",
                  operation.id
              );
              assert_eq!(
                  post.pointer("/responses/501/content/application~1json/schema/$ref"),
                  Some(&json!("#/components/schemas/ApiError")),
                  "{} not-implemented response",
                  operation.id
              );
              destructive = destructive.saturating_add(usize::from(operation.destructive));
          }
          assert!(destructive > 0, "the table marks destructive operations");
          Ok(())
      }
  ```
- [ ] Run: `cargo test --locked -p xtask`
  Expected failure: `list_workspaces not-implemented response` (`left: None`). To see the
  name-list defect on its own, temporarily move the `501` assertion below the loop's other two
  and re-run: the failure becomes `restore_workspace destructive hint`
  (`left: Some(Bool(false))`, `right: Some(Bool(true))`). Put the assertion back.
- [ ] Implement:
  - add after `TRANSPORT_METHODS`:
    ```rust
    /// Statuses that carry `ApiError`; 501 is the generated "not implemented" outcome.
    const ERROR_STATUSES: [&str; 10] = [
        "400", "401", "403", "404", "409", "413", "422", "500", "501", "503",
    ];
    ```
  - in `path_operation`, replace the `for status in [ "400", … "503", ] {` header with
    `for status in ERROR_STATUSES {`;
  - replace `annotations` with:
    ```rust
    fn annotations(info: &OperationInfo) -> Value {
        let read_only = info.permission == Permission::Read;
        json!({
            "readOnlyHint": read_only,
            "destructiveHint": info.destructive,
            "idempotentHint": read_only,
            "openWorldHint": false
        })
    }
    ```
- [ ] Run:
  ```powershell
  cargo fmt -p xtask
  cargo test --locked -p xtask
  cargo run --locked --package xtask -- generate --out "$out\d4"
  Compare-Api "$out\d4"
  ```
  Expected: `5 passed`; `Compare-Api` reports changes in `openapi.json` and `openapi.yaml`
  only (a `501` response per operation; `destructiveHint: true` for `discard_draft`,
  `restore_workspace`, `revoke_connector`).
- [ ] Commit `git add xtask/src/api.rs`:
  ```text
  fix(xtask): read tool hints and the 501 response from the operation table.

  Why: destructiveHint came from a four-name list in the generator, so discard_draft, restore_workspace and revoke_connector were published as non-destructive; ErrorCode::NotImplemented had no declared HTTP response (SPEC 9: "Generate MCP schemas and annotations from the declared operation semantics").
  What changed: annotations() uses OperationInfo.destructive; every operation declares a 501 ApiError response; the id list is deleted.
  Verified: cargo test --locked -p xtask failed before ("list_workspaces not-implemented response"; with that assertion moved, "restore_workspace destructive hint"), 5 passed after; scratch generation differs from api/ only in openapi.json and openapi.yaml.
  Next: declare the MCP App as an rmcp resource.

  Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>
  Claude-Session: https://claude.ai/code/session_01LoZr4ME3crLYjcTNn5GcAC
  ```

---

### Task D.5: `api/mcp-apps.json` is an rmcp resource; the bundle manifest is built from it

**rmcp 3.5.0 shape** (`rmcp-3.5.0/src/model/resource.rs:8-38`):
```rust
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct Resource {
    pub uri: String,
    pub name: String,
    // title, description: Option<String>, skip_serializing_if
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mime_type: Option<String>,
    // size, icons
    #[serde(rename = "_meta", skip_serializing_if = "Option::is_none")]
    pub meta: Option<MetaObject>,
    // annotations
}
```
`uri` and `name` are required to deserialize; `mime_type` is `mimeType` on the wire; `meta` is
`_meta`, and `MetaObject` is `#[serde(transparent)] pub struct MetaObject(pub JsonObject)`
(`model/meta.rs:241-244`). The declaration today has no `name` and puts `csp` at the top level.

**Files:**
- Modify: `xtask/src/api.rs` (`app_resources`, `resource_uri`, constants, `mod tests`)
- Create: `ui/scripts/app-declaration.ts`, `ui/tests/unit/app-declaration.test.ts`
- Modify: `ui/scripts/bundle-app.mjs:9`, `:13-38`, `:80-94`

**Interfaces:**
- Produces `api/mcp-apps.json`:
  ```json
  {"resources":[{"uri":"ui://okf-jawn/app.html","name":"app","mimeType":"text/html;profile=mcp-app",
    "_meta":{"ui":{"csp":{"connectDomains":[],"resourceDomains":[]}}}}]}
  ```
- Produces `ui/dist-apps/manifest.json`: `{"resources":[{…the declared resource…,
  "byteLength": <n>, "sha256": "<hex>"}]}` (the string `csp` field is gone; the harness in
  `qualification/mcp-apps/src/main.rs:35-43` reads `byteLength`, `mimeType`, `name`, `sha256`,
  `uri` and names the file `<name>.html`).
- Produces TS exports:
  ```ts
  export type AppResource = { uri: string; name: string; mimeType: string;
    _meta: { ui: { csp: { connectDomains: string[]; resourceDomains: string[] } } } };
  export function parseAppResource(declaration: unknown, source: string, sdkMimeType: string): AppResource;
  export function manifestEntry(resource: AppResource, byteLength: number, sha256: string):
    AppResource & { byteLength: number; sha256: string };
  ```
- `bundle-app.mjs` reads `process.env.OKF_MCP_APPS` when set, else `../../api/mcp-apps.json`.

- [ ] Write the failing Rust test; add to `mod tests`:
  ```rust
      #[test]
      fn the_app_declaration_is_an_mcp_resource() -> TestResult {
          let directory = tempfile::tempdir()?;
          super::generate(directory.path())?;
          let declaration = read(&directory.path().join("mcp-apps.json"))?;
          let resources = declaration
              .get("resources")
              .and_then(Value::as_array)
              .ok_or("resources")?;
          assert_eq!(resources.len(), 1);
          let resource = resources.first().ok_or("one resource")?;
          assert_eq!(
              resource,
              &json!({
                  "uri": "ui://okf-jawn/app.html",
                  "name": "app",
                  "mimeType": "text/html;profile=mcp-app",
                  "_meta": {"ui": {"csp": {"connectDomains": [], "resourceDomains": []}}}
              })
          );
          let tools = read(&directory.path().join("mcp-tools.json"))?;
          let mut bound = 0_usize;
          for tool in tools
              .get("tools")
              .and_then(Value::as_array)
              .ok_or("tools")?
          {
              if let Some(uri) = tool.pointer("/_meta/ui/resourceUri") {
                  assert_eq!(Some(uri), resource.get("uri"), "tool resource is the declared one");
                  bound = bound.saturating_add(1);
              }
          }
          assert!(bound > 0, "at least one tool renders in the App");
          Ok(())
      }
  ```
- [ ] Run: `cargo test --locked -p xtask` → expected failure: `left` has `"csp"` at the top
  level and no `"name"`.
- [ ] Implement in `xtask/src/api.rs`:
  - add after `ERROR_STATUSES`:
    ```rust
    /// The one shared MCP App resource; `structuredContent` selects the feature surface.
    const APP_RESOURCE_NAME: &str = "app";
    const APP_RESOURCE_URI: &str = "ui://okf-jawn/app.html";
    const APP_RESOURCE_MIME_TYPE: &str = "text/html;profile=mcp-app";
    ```
  - replace `app_resources` with:
    ```rust
    /// The MCP App resources this build declares, in the shape `rmcp::model::Resource` reads.
    fn app_resources() -> Value {
        json!({
            "resources": [{
                "uri": APP_RESOURCE_URI,
                "name": APP_RESOURCE_NAME,
                "mimeType": APP_RESOURCE_MIME_TYPE,
                "_meta": {"ui": {"csp": {"connectDomains": [], "resourceDomains": []}}}
            }]
        })
    }
    ```
  - replace `resource_uri` with:
    ```rust
    fn resource_uri(key: &str) -> Option<&'static str> {
        (!key.is_empty()).then_some(APP_RESOURCE_URI)
    }
    ```
- [ ] Run: `cargo fmt -p xtask; cargo test --locked -p xtask` → `6 passed`.
- [ ] Write the failing UI test. Create `ui/tests/unit/app-declaration.test.ts`:
  ```ts
  /** The bundle manifest comes from the generated declaration and fails on disagreement. */

  import { RESOURCE_MIME_TYPE } from '@modelcontextprotocol/ext-apps';
  import { describe, expect, it } from 'vitest';
  import { manifestEntry, parseAppResource } from '../../scripts/app-declaration';

  const resource = {
    uri: 'ui://okf-jawn/app.html',
    name: 'app',
    mimeType: RESOURCE_MIME_TYPE,
    _meta: { ui: { csp: { connectDomains: [], resourceDomains: [] } } },
  };
  const declaration = { resources: [resource] };

  function withResource(change: Record<string, unknown>) {
    return { resources: [{ ...resource, ...change }] };
  }

  describe('parseAppResource', () => {
    it('returns the declared resource and builds the manifest entry from it', () => {
      const parsed = parseAppResource(declaration, 'mcp-apps.json', RESOURCE_MIME_TYPE);
      expect(manifestEntry(parsed, 12, 'ab'.repeat(32))).toEqual({
        ...resource,
        byteLength: 12,
        sha256: 'ab'.repeat(32),
      });
    });

    it('fails when the uri is not the file the build writes', () => {
      expect(() =>
        parseAppResource(
          withResource({ uri: 'ui://okf-jawn/other.html' }),
          'mcp-apps.json',
          RESOURCE_MIME_TYPE,
        ),
      ).toThrow(/uri ui:\/\/okf-jawn\/other\.html !== built resource ui:\/\/okf-jawn\/app\.html/);
    });

    it('fails when the mimeType is not the installed SDK profile', () => {
      expect(() =>
        parseAppResource(
          withResource({ mimeType: 'text/html' }),
          'mcp-apps.json',
          RESOURCE_MIME_TYPE,
        ),
      ).toThrow(/mimeType text\/html !== SDK/);
    });

    it('fails when csp is a string instead of the declared domain lists', () => {
      expect(() =>
        parseAppResource(
          withResource({ _meta: { ui: { csp: "default-src 'none'" } } }),
          'mcp-apps.json',
          RESOURCE_MIME_TYPE,
        ),
      ).toThrow(/not an MCP App declaration/);
    });

    it('fails when the resource has no name', () => {
      const { name: _name, ...unnamed } = resource;
      expect(() =>
        parseAppResource({ resources: [unnamed] }, 'mcp-apps.json', RESOURCE_MIME_TYPE),
      ).toThrow(/not an MCP App declaration/);
    });
  });
  ```
- [ ] Run: `bun --bun run --cwd ui test tests/unit/app-declaration.test.ts`
  Expected failure: `Failed to resolve import "../../scripts/app-declaration"`.
- [ ] Implement. Create `ui/scripts/app-declaration.ts`:
  ```ts
  /** The generated MCP App resource declaration is the only source of the bundle manifest. */
  import { z } from 'zod';

  const domains = z.array(z.string());
  const resourceSchema = z.strictObject({
    uri: z.string().min(1),
    name: z.string().regex(/^[a-z][a-z0-9-]*$/),
    mimeType: z.string().min(1),
    _meta: z.strictObject({
      ui: z.strictObject({
        csp: z.strictObject({ connectDomains: domains, resourceDomains: domains }),
      }),
    }),
  });
  const declarationSchema = z.strictObject({ resources: z.tuple([resourceSchema]) });

  /** One declared App resource, in the shape `rmcp::model::Resource` deserializes. */
  export type AppResource = z.infer<typeof resourceSchema>;

  /**
   * Return the single declared resource. Throws when the declaration is malformed, when its uri is
   * not the file this build writes, or when its mimeType is not the installed SDK's.
   */
  export function parseAppResource(
    declaration: unknown,
    source: string,
    sdkMimeType: string,
  ): AppResource {
    const parsed = declarationSchema.safeParse(declaration);
    if (!parsed.success) {
      throw new Error(`${source} is not an MCP App declaration: ${parsed.error.message}`);
    }
    const [resource] = parsed.data.resources;
    const builtUri = `ui://okf-jawn/${resource.name}.html`;
    if (resource.uri !== builtUri) {
      throw new Error(`${source} uri ${resource.uri} !== built resource ${builtUri}`);
    }
    if (resource.mimeType !== sdkMimeType) {
      throw new Error(`${source} mimeType ${resource.mimeType} !== SDK ${sdkMimeType}`);
    }
    return resource;
  }

  /** The manifest entry for the built file: the declaration plus the bytes' length and digest. */
  export function manifestEntry(resource: AppResource, byteLength: number, sha256: string) {
    return { ...resource, byteLength, sha256 };
  }
  ```
- [ ] Implement in `ui/scripts/bundle-app.mjs`:
  - `:9` (`* URI and mimeType must match the committed api/mcp-apps.json declaration.`) →
    ```js
     * The manifest is built from the generated api/mcp-apps.json alone (OKF_MCP_APPS overrides the
     * path for a scratch generation); a uri or mimeType that disagrees with this build fails it.
    ```
  - after `import { mkdir, readFile, writeFile } from 'node:fs/promises';` add
    `import { env } from 'node:process';`
  - after `import { build } from 'vite';` add
    `import { manifestEntry, parseAppResource } from './app-declaration.ts';`
  - replace `:21-38` (from `const declarationPath = …` through the `mimeType` check) with:
    ```js
    const declarationPath =
      env.OKF_MCP_APPS ?? fileURLToPath(new URL('../../api/mcp-apps.json', import.meta.url));
    const declared = parseAppResource(
      JSON.parse(await readFile(declarationPath, 'utf8')),
      declarationPath,
      RESOURCE_MIME_TYPE,
    );
    ```
  - replace `:80-94` (from `const file = 'dist-apps/app.html';` to the end of the file) with:
    ```js
    const file = `dist-apps/${declared.name}.html`;
    await writeFile(file, html);
    const bytes = Buffer.from(html, 'utf8');
    const resources = [
      manifestEntry(declared, bytes.byteLength, createHash('sha256').update(bytes).digest('hex')),
    ];
    await writeFile('dist-apps/manifest.json', `${JSON.stringify({ resources }, null, 2)}\n`);
    process.stdout.write(`Built shared ${declared.uri} MCP App resource.\n`);
    ```
  The Content-Security-Policy `<meta>` in the HTML (`:77-78`) is unchanged; only the manifest
  stops repeating it as a string.
- [ ] Run:
  ```powershell
  bun --bun run --cwd ui test tests/unit/app-declaration.test.ts
  bun --bun run --cwd ui typecheck
  cargo run --locked --package xtask -- generate --out "$out\d5"
  $env:OKF_MCP_APPS = "$out\d5\api\mcp-apps.json"; bun --bun run --cwd ui build; Remove-Item Env:OKF_MCP_APPS
  Get-Content ui\dist-apps\manifest.json
  bun --bun run --cwd ui build
  ```
  Expected: `5 passed`; typecheck exits 0; the first build ends with
  `Built shared ui://okf-jawn/app.html MCP App resource.` and the manifest lists one resource
  with `uri`, `name`, `mimeType`, `_meta.ui.csp.{connectDomains,resourceDomains}`,
  `byteLength`, `sha256` and no top-level `csp`; the second build (committed, old-shape
  `api/mcp-apps.json`) fails with `mcp-apps.json is not an MCP App declaration` — expected on
  this branch until the orchestrator regenerates (Deviations 9).
- [ ] Commit `git add xtask/src/api.rs ui/scripts/app-declaration.ts ui/scripts/bundle-app.mjs ui/tests/unit/app-declaration.test.ts`:
  ```text
  feat(xtask,ui): declare the MCP App as an rmcp resource and build the manifest from it.

  Why: api/mcp-apps.json had no name and a top-level csp, so it could not deserialize as rmcp 3.5 Resource (uri and name required, _meta for extensions); bundle-app.mjs wrote its own manifest with csp as a string and a hard-coded uri (design section 5, D).
  What changed: the declaration is {uri, name, mimeType, _meta.ui.csp.{connectDomains,resourceDomains}}; bundle-app.mjs parses that file (OKF_MCP_APPS overrides the path), fails when uri or mimeType disagree with what it builds, and writes manifest entries as the declaration plus byteLength and sha256.
  Verified: cargo test --locked -p xtask 6 passed (the new test failed on the old shape); bun --bun run --cwd ui test tests/unit/app-declaration.test.ts 5 passed (failed to resolve the module before); ui build against a scratch declaration writes the manifest; ui build against the committed old-shape file fails as designed.
  Next: catalog.json and the single spec shape. Blocked for the committed outputs only: orchestrator regenerates api/mcp-apps.json at merge.

  Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>
  Claude-Session: https://claude.ai/code/session_01LoZr4ME3crLYjcTNn5GcAC
  ```

---

### Task D.6: `api/presentation/catalog.json`; one definition of the json-render spec shape

**Files:**
- Create: `ui/src/features/views/spec-schema.ts`, `ui/scripts/catalog-response.ts`,
  `ui/tests/unit/catalog-response.test.ts`
- Modify: `ui/src/features/views/Layout.tsx:11`, `:16`, `:76-94`, `:115`
- Modify: `ui/scripts/catalog-entry.ts` (whole file), `scripts/lib/generation.mjs:11`, new export

**Interfaces:**
- Consumes: `catalog` from `ui/src/features/views/catalog.ts` (unchanged); `zCatalogResponse`
  from `ui/src/api/generated/zod.gen.ts`; `z.toJSONSchema(schema, { target: 'draft-7' })`
  (zod 4.6.5); the Rust generator's `api/forms/view.schema.json`.
- Produces `api/presentation/catalog.json`, the contract's `CatalogResponse`
  (`crates/contract/src/views.rs:137-161`):
  ```json
  {"schema_version": 1,
   "components": [{"name": "Stack", "description": "…", "properties_schema": {"type": "object", "properties": {…}, "additionalProperties": false}, "actions": []}, …],
   "view_schema": { …api/forms/view.schema.json… }}
  ```
- Produces TS exports:
  ```ts
  // ui/src/features/views/spec-schema.ts
  export const slotsSchema, repeatSchema, rawElementSchema, rawSpecSchema;
  export type RawElement;
  export interface SpecComponent { name: string; props: z.ZodObject }
  export function storedElementSchema(component: SpecComponent);
  export function storedSpecSchema(components: readonly SpecComponent[]);
  export function storedSpecJsonSchema(components: readonly SpecComponent[]): Record<string, unknown>;
  // ui/scripts/catalog-response.ts
  export interface CatalogDocument { schema_version: number; components: {…}[]; view_schema: unknown }
  export const CATALOG_SCHEMA_VERSION = 1;
  export function specComponents(): (SpecComponent & { description: string })[];
  export function specJsonSchema(): Record<string, unknown>;
  export function catalogResponse(viewSchema: unknown): CatalogDocument;
  // scripts/lib/generation.mjs
  export async function generateInto(root, output);   // one pass into `output`, nothing replaced
  ```
- New generator input: `ui/scripts/generate-catalog.mjs` is run with
  `OKF_VIEW_SCHEMA=<output>/api/forms/view.schema.json` beside `OKF_CATALOG_OUT`.
- `api/presentation/catalog.schema.json` is now derived from the Zod definition (same
  constraints; `anyOf` instead of `oneOf`, plus `propertyNames` and `"type": "string"` beside
  each `const`).

- [ ] Write the failing test. Create `ui/tests/unit/catalog-response.test.ts`:
  ```ts
  /** The generated catalog document conforms to the contract's CatalogResponse. */

  import type { RJSFSchema } from '@rjsf/utils';
  import { customizeValidator } from '@rjsf/validator-ajv8';
  import { describe, expect, it } from 'vitest';
  import viewSchema from '../../../api/forms/view.schema.json';
  import { catalogResponse, specJsonSchema } from '../../scripts/catalog-response';
  import { zCatalogResponse } from '../../src/api/generated/zod.gen';
  import { catalog } from '../../src/features/views/catalog';
  import sixComponentSpec from '../fixtures/six-component-spec.json';

  const validator = customizeValidator();

  describe('catalog.json', () => {
    it('parses as CatalogResponse without losing or inventing a field', () => {
      const document = catalogResponse(viewSchema);
      expect(zCatalogResponse.parse(document)).toEqual(document);
      expect(Object.keys(document).sort()).toEqual(['components', 'schema_version', 'view_schema']);
    });

    it('describes every approved component with closed props and its actions', () => {
      const document = catalogResponse(viewSchema);
      expect(document.components.map((component) => component.name)).toEqual(catalog.componentNames);
      for (const component of document.components) {
        expect(Object.keys(component).sort()).toEqual([
          'actions',
          'description',
          'name',
          'properties_schema',
        ]);
        expect(component.description.length).toBeGreaterThan(0);
        expect(component.properties_schema).toMatchObject({
          type: 'object',
          additionalProperties: false,
        });
        expect(component.properties_schema).not.toHaveProperty('$schema');
        expect(component.actions).toEqual(catalog.actionNames);
      }
    });

    it('carries the persisted View document schema unchanged', () => {
      expect(catalogResponse(viewSchema).view_schema).toEqual(viewSchema);
    });
  });

  describe('stored spec schema derived from the single definition', () => {
    const schema = specJsonSchema() as RJSFSchema;

    it('accepts the six-component fixture', () => {
      expect(validator.validateFormData(sixComponentSpec, schema).errors).toEqual([]);
    });

    it('rejects props the component does not declare', () => {
      const result = validator.validateFormData(
        {
          root: 'root',
          elements: {
            root: {
              type: 'Chart',
              props: { binding: 'metrics', chart: 'metrics_chart', title: 'Metrics', extra: true },
              children: [],
            },
          },
        },
        schema,
      );
      expect(result.errors.length).toBeGreaterThan(0);
    });

    it('rejects a component that is not in the catalog', () => {
      const result = validator.validateFormData(
        { root: 'root', elements: { root: { type: 'NotInCatalog', props: {}, children: [] } } },
        schema,
      );
      expect(result.errors.length).toBeGreaterThan(0);
    });

    it('rejects an element without children, which the stored shape requires', () => {
      const result = validator.validateFormData(
        { root: 'root', elements: { root: { type: 'Columns', props: {} } } },
        schema,
      );
      expect(result.errors.length).toBeGreaterThan(0);
    });
  });
  ```
- [ ] Run: `bun --bun run --cwd ui test tests/unit/catalog-response.test.ts`
  Expected failure: `Failed to resolve import "../../scripts/catalog-response"`.
- [ ] Implement. Create `ui/src/features/views/spec-schema.ts`:
  ```ts
  /**
   * The json-render spec shape, defined once.
   *
   * `rawSpecSchema` is what an author or agent may hand in: slots as a record or a plain array,
   * children optional. `storedSpecSchema` is the canonical shape a saved View carries, closed per
   * approved component. The published JSON Schema is derived from the stored shape; nothing here is
   * restated by hand anywhere else.
   */
  import { z } from 'zod';

  export const slotsSchema = z.record(z.string(), z.array(z.string()));
  export const repeatSchema = z.strictObject({
    statePath: z.union([z.string(), z.object({ $item: z.string() })]),
    key: z.string().optional(),
  });
  export const rawElementSchema = z.strictObject({
    type: z.string().min(1),
    props: z.record(z.string(), z.unknown()).default({}),
    children: z.array(z.string()).optional(),
    slots: z.unknown().optional(),
    visible: z.unknown().optional(),
    repeat: z.unknown().optional(),
  });
  export const rawSpecSchema = z.strictObject({
    root: z.string().min(1),
    elements: z.record(z.string(), rawElementSchema),
    state: z.record(z.string(), z.unknown()).optional(),
  });
  export type RawElement = z.infer<typeof rawElementSchema>;

  /** One approved component: its catalog name and the object schema of its props. */
  export interface SpecComponent {
    name: string;
    props: z.ZodObject;
  }

  /** The stored element of one approved component; unknown props and unknown keys are rejected. */
  export function storedElementSchema(component: SpecComponent) {
    return z.strictObject({
      type: z.literal(component.name),
      props: z.strictObject(component.props.shape),
      children: z.array(z.string()),
      slots: slotsSchema.optional(),
      visible: z.unknown().optional(),
      repeat: z.unknown().optional(),
    });
  }

  /** The stored spec over the approved components. */
  export function storedSpecSchema(components: readonly SpecComponent[]) {
    const [first, ...rest] = components.map(storedElementSchema);
    if (first === undefined) throw new Error('A stored spec needs at least one approved component');
    return z.strictObject({
      root: z.string(),
      elements: z.record(z.string(), z.union([first, ...rest])),
      state: z.record(z.string(), z.unknown()).optional(),
    });
  }

  /** Draft-07 JSON Schema of the stored spec, for validators that do not run Zod. */
  export function storedSpecJsonSchema(
    components: readonly SpecComponent[],
  ): Record<string, unknown> {
    return z.toJSONSchema(storedSpecSchema(components), { target: 'draft-7' });
  }
  ```
- [ ] Implement in `ui/src/features/views/Layout.tsx` (import-only change; behaviour and
  exports are the same):
  - delete `:11` (`import { z } from 'zod';`)
  - after `:16` (`import { DataTable } from './DataTable';`) add
    `import { type RawElement, rawSpecSchema, repeatSchema, slotsSchema } from './spec-schema';`
  - delete `:76-94` (the four `const … = z.…` definitions `slotsSchema`, `repeatSchema`,
    `rawElementSchema`, `rawSpecSchema` and the blank line after them)
  - `:115` → `function normalizeElement(key: string, raw: RawElement): UIElement {`
- [ ] Create `ui/scripts/catalog-response.ts`:
  ```ts
  /** The published catalog document: the contract's `CatalogResponse`, built from the real catalog. */
  import { z } from 'zod';
  import { catalog } from '../src/features/views/catalog';
  import { type SpecComponent, storedSpecJsonSchema } from '../src/features/views/spec-schema';

  /** Wire shape of `CatalogResponse`; the generated Zod schema is the conformance check. */
  export interface CatalogDocument {
    schema_version: number;
    components: {
      name: string;
      description: string;
      properties_schema: Record<string, unknown>;
      actions: string[];
    }[];
    view_schema: unknown;
  }

  export const CATALOG_SCHEMA_VERSION = 1;

  /** Every approved component with its description, in catalog order. */
  export function specComponents(): (SpecComponent & { description: string })[] {
    return Object.entries(catalog.data.components).map(([name, entry]) => ({
      name,
      description: entry.description,
      props: entry.props,
    }));
  }

  /** JSON Schema of the stored json-render spec for this catalog. */
  export function specJsonSchema(): Record<string, unknown> {
    return storedSpecJsonSchema(specComponents());
  }

  /** Build the catalog document; `viewSchema` is the generated schema of a persisted View document. */
  export function catalogResponse(viewSchema: unknown): CatalogDocument {
    return {
      schema_version: CATALOG_SCHEMA_VERSION,
      components: specComponents().map(({ name, description, props }) => {
        const { $schema: _dialect, ...properties } = z.toJSONSchema(z.strictObject(props.shape), {
          target: 'draft-7',
        });
        return {
          name,
          description,
          properties_schema: properties,
          actions: [...catalog.actionNames],
        };
      }),
      view_schema: viewSchema,
    };
  }
  ```
- [ ] Replace `ui/scripts/catalog-entry.ts` with:
  ```ts
  /** Emit the catalog's published files from json-render's library APIs and the single spec shape. */
  import { mkdirSync, readFileSync, writeFileSync } from 'node:fs';
  import { join } from 'node:path';
  import { env } from 'node:process';
  import { catalog } from '../src/features/views/catalog';
  import { catalogResponse, specJsonSchema } from './catalog-response';

  // Read through node:process so the bundler cannot fold these at build time.
  const output = env.OKF_CATALOG_OUT;
  if (!output) throw new Error('OKF_CATALOG_OUT must name a generation staging directory');
  const viewSchemaPath = env.OKF_VIEW_SCHEMA;
  if (!viewSchemaPath)
    throw new Error('OKF_VIEW_SCHEMA must name the generated forms/view.schema.json');
  const viewSchema: unknown = JSON.parse(readFileSync(viewSchemaPath, 'utf8'));

  function writeJson(name: string, value: unknown): void {
    writeFileSync(join(output as string, name), `${JSON.stringify(value, null, 2)}\n`);
  }

  mkdirSync(output, { recursive: true });
  writeJson('catalog.schema.json', specJsonSchema());
  writeJson('catalog.json', catalogResponse(viewSchema));
  writeFileSync(
    join(output, 'catalog-prompt.txt'),
    `${catalog.prompt({
      customRules: [
        'Source components receive binding names, never model-written quotations or data.',
        'Use only the resolved sources provided by the application. Missing bindings must remain unresolved.',
        'Presentation does not grant permission to modify, approve, or verify content.',
      ],
    })}\n`,
  );
  writeJson('catalog-components.json', {
    components: catalog.componentNames,
    actions: catalog.actionNames,
  });
  ```
- [ ] Implement in `scripts/lib/generation.mjs`:
  - `:11` →
    ```js
      await run(bun(), ['scripts/generate-catalog.mjs'], { cwd: join(root, 'ui'), env: { OKF_CATALOG_OUT: join(output, 'api', 'presentation'), OKF_VIEW_SCHEMA: join(output, 'api', 'forms', 'view.schema.json') } });
    ```
  - after `requireLockfiles` (before `export async function generate`) add:
    ```js
    /** Run both generators once into `output`; the committed generated directories are not touched. */
    export async function generateInto(root, output) {
      await requireLockfiles(root);
      await onePass(root, output);
    }
    ```
  (`tests/foundation/generation.test.mjs` matches `onePass(root, first)`, `onePass(root,
  second)`, `cargo`, `openapi-ts` and `generate-catalog` in this file's source; all remain.)
- [ ] Run:
  ```powershell
  bun --bun run --cwd ui test
  bun --bun run --cwd ui typecheck
  Set-Location ui; bun x biome check scripts src/features/views tests/unit/app-declaration.test.ts tests/unit/catalog-response.test.ts; Set-Location ..
  Remove-Item -Recurse -Force "$out\d6" -ErrorAction SilentlyContinue
  $env:OKF_SCRATCH = "$out\d6"
  bun -e "import('./scripts/lib/generation.mjs').then(m => m.generateInto(process.cwd(), process.env.OKF_SCRATCH)).catch(e => { console.error(e); process.exit(1); })"
  Get-ChildItem "$out\d6\api\presentation" | Select-Object -ExpandProperty Name
  bun -e "const d=JSON.parse(require('fs').readFileSync(process.env.OKF_SCRATCH+'/api/presentation/catalog.json','utf8')); console.log(Object.keys(d).join(), d.schema_version, d.components.map(c=>c.name).join(), typeof d.view_schema.properties.bindings)"
  git status --porcelain
  ```
  Expected: Vitest `Test Files 10 passed` (the eight existing files, `app-declaration` and
  `catalog-response` with 7 tests; `layout.test.tsx` and `json-render-roundtrip.test.tsx` still
  pass through the moved schemas); typecheck exits 0; biome reports `Checked … No fixes
  applied` with no errors; the presentation directory lists `catalog-components.json`,
  `catalog-prompt.txt`, `catalog.json`, `catalog.schema.json`, `vega-lite.schema.json`; the
  summary line is
  `schema_version,components,view_schema 1 Stack,Columns,SourceExcerpt,DataTable,Chart,SourceList object`;
  `git status` shows only D's authored files (nothing under `api/`).
- [ ] Rule-removal check (no commit): in `ui/scripts/catalog-response.ts` change
  `z.strictObject(props.shape)` to `z.looseObject(props.shape)`; run
  `bun --bun run --cwd ui test tests/unit/catalog-response.test.ts` → `describes every approved
  component with closed props and its actions` fails. Restore.
- [ ] Commit `git add ui/src/features/views/spec-schema.ts ui/src/features/views/Layout.tsx ui/scripts/catalog-response.ts ui/scripts/catalog-entry.ts ui/tests/unit/catalog-response.test.ts scripts/lib/generation.mjs`:
  ```text
  feat(ui): generate catalog.json as CatalogResponse from one definition of the spec shape.

  Why: get_catalog's response type had no generated output, and the json-render spec shape was written three times (catalog-entry.ts by hand as JSON Schema, Layout.tsx as Zod, and by the library) (design section 5, D; section 6 "Catalog schema with per-component props").
  What changed: features/views/spec-schema.ts defines the raw and stored spec shapes; Layout.tsx imports the raw shape; catalog.schema.json is derived from the stored shape with z.toJSONSchema; new api/presentation/catalog.json carries schema_version, per-component name/description/properties_schema/actions and view_schema (the generated ViewDocument form schema, passed as OKF_VIEW_SCHEMA). generation.mjs exports generateInto(root, output).
  Verified: bun --bun run --cwd ui test passes (10 files; catalog-response failed to resolve its module before); bun --bun run --cwd ui typecheck exits 0; generateInto writes catalog.json whose Zod parse equals the document; loosening the props schema fails the test.
  Next: create_sandbox_capability must not be a cached query.

  Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>
  Claude-Session: https://claude.ai/code/session_01LoZr4ME3crLYjcTNn5GcAC
  ```

---

### Task D.7: `create_sandbox_capability` is not generated as a cached query

**Files:** Modify `ui/openapi-ts.config.ts:9-21`.

**Interfaces:**
- Consumes Hey API 0.99.0 `parser.hooks.operations.getKind(operation)`: a `['query']` result
  makes a query; `undefined` falls through to the default, which makes a POST a mutation
  (`@hey-api/shared` `isOperationKind`: hooks are tried in order and the first non-`undefined`
  result wins).
- Produces in `ui/src/api/generated/@tanstack/react-query.gen.ts` (after regeneration):
  `createSandboxCapabilityMutation`; no `createSandboxCapabilityOptions`, no
  `createSandboxCapabilityQueryKey`.

- [ ] Show the defect in the scratch output of D.6 (generated before this change):
  ```powershell
  Select-String -Path "$out\d6\client\@tanstack\react-query.gen.ts" -Pattern 'export const createSandboxCapability\w+' | ForEach-Object { $_.Matches[0].Value }
  ```
  Expected: `export const createSandboxCapabilityQueryKey` and
  `export const createSandboxCapabilityOptions` (a capability URL that expires would be served
  from the query cache).
- [ ] Implement in `ui/openapi-ts.config.ts`. Replace `:9-21` with:
  ```ts
  /** Reads that mint a new credential on every call; a cached query would hand back an expiring one. */
  const neverCached = new Set(['create_sandbox_capability']);
  const readPaths = new Set<string>();
  for (const value of metadata) {
    if (
      typeof value !== 'object' ||
      value === null ||
      !('id' in value) ||
      !('path' in value) ||
      !('permission' in value)
    ) {
      throw new Error('Invalid generated operation metadata');
    }
    if (typeof value.path !== 'string') throw new Error('Operation path must be a string');
    if (value.permission === 'read' && !(typeof value.id === 'string' && neverCached.has(value.id)))
      readPaths.add(value.path);
  }
  ```
  (`getKind` at `:29` is unchanged: a path not in `readPaths` returns `undefined`.)
- [ ] Run:
  ```powershell
  bun --bun run --cwd ui typecheck
  Remove-Item -Recurse -Force "$out\d7" -ErrorAction SilentlyContinue
  $env:OKF_SCRATCH = "$out\d7"
  bun -e "import('./scripts/lib/generation.mjs').then(m => m.generateInto(process.cwd(), process.env.OKF_SCRATCH)).catch(e => { console.error(e); process.exit(1); })"
  Select-String -Path "$out\d7\client\@tanstack\react-query.gen.ts" -Pattern 'export const createSandboxCapability\w+' | ForEach-Object { $_.Matches[0].Value }
  (Select-String -Path "$out\d6\client\@tanstack\react-query.gen.ts" -Pattern 'QueryKey = ').Count
  (Select-String -Path "$out\d7\client\@tanstack\react-query.gen.ts" -Pattern 'QueryKey = ').Count
  ```
  Expected: typecheck exits 0; the only match is
  `export const createSandboxCapabilityMutation`; the query-key count drops by exactly one
  (no other read changed kind).
- [ ] Commit `git add ui/openapi-ts.config.ts`:
  ```text
  fix(ui): generate create_sandbox_capability as a mutation, not a cached query.

  Why: the client generator made every read-permission operation a TanStack query; create_sandbox_capability mints a short-lived capability URL on each call, so a cached result would replay an expired credential (SPEC 11: sandboxed separate origin, no ambient credentials).
  What changed: openapi-ts.config.ts keeps a one-entry set of read operations that are never cached and leaves them to the default kind (POST: mutation).
  Verified: generating into a scratch directory before the change emits createSandboxCapabilityOptions and createSandboxCapabilityQueryKey; after it emits only createSandboxCapabilityMutation and exactly one fewer query key; bun --bun run --cwd ui typecheck exits 0.
  Next: package gate; orchestrator regenerates at merge.

  Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>
  Claude-Session: https://claude.ai/code/session_01LoZr4ME3crLYjcTNn5GcAC
  ```

---

### Package D acceptance

Run in `D:\okf\cure\generator` from PowerShell at the tip of `cure/generator`, tree clean.
`<C>` is the base commit (`git merge-base HEAD integration/foundation-cure`).

1. Scope.
   ```powershell
   git status --porcelain
   git diff --name-only <C>..HEAD
   ```
   Expected: no status output. The names are exactly `Cargo.lock`, `Cargo.toml`,
   `scripts/lib/generation.mjs`, `ui/openapi-ts.config.ts`, `ui/scripts/app-declaration.ts`,
   `ui/scripts/bundle-app.mjs`, `ui/scripts/catalog-entry.ts`, `ui/scripts/catalog-response.ts`,
   `ui/src/features/views/Layout.tsx`, `ui/src/features/views/spec-schema.ts`,
   `ui/tests/unit/app-declaration.test.ts`, `ui/tests/unit/catalog-response.test.ts`,
   `xtask/Cargo.toml`, `xtask/src/api.rs`. Nothing under `api/`, `generated/` or
   `ui/src/api/generated/`.
2. Rust gate.
   ```powershell
   cargo fmt -p xtask --check
   cargo clippy --locked -p xtask --all-targets -- -D warnings
   cargo test --locked -p xtask
   cargo run --locked --package xtask -- source-policy --root .
   ```
   Expected: all exit 0; the test line is `test result: ok. 6 passed`.
3. `utoipa` is gone and the lockfile only lost lines.
   ```powershell
   git grep -n utoipa -- Cargo.toml Cargo.lock xtask
   git diff <C>..HEAD -- Cargo.lock | Select-String -Pattern '^\+(?!\+\+)'
   git diff <C>..HEAD -- Cargo.lock | Select-String -Pattern '^-name = '
   ```
   Expected: no output; no output; exactly `-name = "utoipa"` and `-name = "utoipa-gen"`.
4. No rename machinery and no literal component names remain.
   ```powershell
   git grep -nE "Input|Output|splits|components/schemas/[A-Z]" -- xtask/src/api.rs
   ```
   Expected: one line, the `"#/components/schemas/ApiError"` assertion inside `mod tests`.
5. UI gate.
   ```powershell
   bun --bun run --cwd ui test
   bun --bun run --cwd ui typecheck
   Set-Location ui; bun x biome check scripts src/features/views tests/unit/app-declaration.test.ts tests/unit/catalog-response.test.ts openapi-ts.config.ts; Set-Location ..
   ```
   Expected: 10 test files pass; typecheck exits 0; biome reports no errors (two
   `useLiteralKeys` infos in `openapi-ts.config.ts` exist at base).
6. Deterministic scratch generation.
   ```powershell
   $base = 'D:\okf\cure\generator-out'
   foreach ($run in 'accept','accept2') {
     Remove-Item -Recurse -Force "$base\$run" -ErrorAction SilentlyContinue
     $env:OKF_SCRATCH = "$base\$run"
     bun -e "import('./scripts/lib/generation.mjs').then(m => m.generateInto(process.cwd(), process.env.OKF_SCRATCH)).catch(e => { console.error(e); process.exit(1); })"
   }
   git diff --no-index --stat "$base\accept" "$base\accept2"
   ```
   Expected: both runs exit 0; the diff prints nothing.
7. What regeneration will change, and nothing else.
   ```powershell
   git diff --no-index --stat api "$base\accept\api"
   git diff --no-index --stat generated/cli "$base\accept\cli"
   git diff --no-index --stat ui/src/api/generated "$base\accept\client"
   ```
   Expected:
   - `api`: exactly `mcp-apps.json` (new shape), `openapi.json` and `openapi.yaml` (a `501`
     response per operation; `destructiveHint: true` for `discard_draft`, `restore_workspace`,
     `revoke_connector`), `presentation/catalog.json` (new), `presentation/catalog.schema.json`
     (derived from Zod). `schemas/`, `forms/`, `examples/`, `operations.json`,
     `transports.json`, `mcp-tools.json`, `presentation/catalog-prompt.txt`,
     `presentation/catalog-components.json`, `presentation/vega-lite.schema.json` are identical.
   - `generated/cli`: no output.
   - client: `@tanstack/react-query.gen.ts` (the sandbox capability becomes a mutation) and
     the files that enumerate per-operation error responses (`types.gen.ts`; `zod.gen.ts` and
     `sdk.gen.ts` only if they list them): a `501` entry. No other change.
8. The capability is not a cached query.
   ```powershell
   Select-String -Path "$base\accept\client\@tanstack\react-query.gen.ts" -Pattern 'export const createSandboxCapability\w+' | ForEach-Object { $_.Matches[0].Value }
   ```
   Expected: only `export const createSandboxCapabilityMutation`.
9. The bundle manifest is built from the declaration.
   ```powershell
   $env:OKF_MCP_APPS = "$base\accept\api\mcp-apps.json"; bun --bun run --cwd ui build; Remove-Item Env:OKF_MCP_APPS
   Get-Content ui\dist-apps\manifest.json
   ```
   Expected: the build succeeds; one resource with `uri`, `name` `app`, `mimeType`,
   `_meta.ui.csp.connectDomains`, `_meta.ui.csp.resourceDomains`, `byteLength`, `sha256`; no
   top-level `csp`.
10. Rule removal: a type with two wire shapes stops generation.
    In `crates/contract/src/history.rs` delete the
    `#[serde(default, skip_serializing_if = "Option::is_none")]` line above `old_path`, then:
    ```powershell
    cargo run --locked --package xtask -- generate --out "$base\split"
    git checkout -- crates/contract/src/history.rs
    ```
    Expected: the generator exits non-zero and prints `FileChange: serialize and deserialize
    schemas differ (reached through Job); give the type one wire shape or two named types`
    (`Job` is the first table type that contains `FileChange`, through
    `ApiError → ErrorDetail → DraftConflictItem`).

Red on this branch by design until the orchestrator regenerates at merge: `bun scripts/dev.mjs
gen-check`, and `bun --bun run --cwd ui build` without `OKF_MCP_APPS` (Deviations 9).

### Deviations

1. **Root `Cargo.toml` and `Cargo.lock` are integration-owner files** (AGENTS.md: builders
   "must not alter … dependencies"). D edits them only because this package's requirement 5
   says to: two deleted lines in `Cargo.toml`, and a lockfile diff that only removes `utoipa`
   and `utoipa-gen`. Stale after it, in files D may not touch: `vendors.json:8-33` (the
   `utoipa` entry; `tests/foundation/vendor.test.mjs` still passes because its `use_sites`
   exist), `README.md:75` (`vendor utoipa`), `.agents/skills/regenerate-api/SKILL.md:8`.
2. **The agreement test follows this package's brief, not the design's wording.** Design §5 D
   says "a synthesized instance of every request and response is validated against its
   per-operation schema and its OpenAPI component". The brief asks for: every per-operation
   schema compiles, every `$ref` resolves, every generated example validates against the
   operation it names. D implements the brief. Synthesized requests decoded into their Rust
   types already exist in `crates/contract/tests/scope.rs:59-153`; nothing synthesizes
   response instances. If the orchestrator wants the design's version, it is a new task.
3. **No shared test helpers.** `tests/support/check.rs` is produced by package E, in parallel.
   D's Rust tests declare `type TestResult` locally and use `let … else { return Err(…) }`.
4. **rmcp cannot check the declaration from `xtask`.** `xtask` does not depend on `rmcp` and D
   may not add dependencies, so D's test asserts the JSON shape rmcp needs. The decode itself,
   `serde_json::from_value::<Vec<rmcp::model::Resource>>(document["resources"])`, belongs in
   `crates/mcp` (package G or the integrate step).
5. **One id match stays in `xtask`.** `transport_response_schema` maps three transport ids to
   response types because `okf_jawn_contract::transport::TransportOperation`
   (`crates/contract/src/transport.rs:19-38`) has no response-type column. Removing the match
   needs a contract change (a typed column or a macro table), which is the integration owner's.
6. **`create_sandbox_capability` is excluded by a one-id set in the config.** No table column
   says "this read mints a credential; never cache it". If more such operations appear, a
   column is the cure; D cannot add one.
7. **`view_schema` is the persisted View document schema, not the json-render spec schema.**
   `CatalogResponse.view_schema` is documented "Schema for persisted View documents"
   (`crates/contract/src/views.rs:159-160`), so `catalog.json` embeds the generated draft-07
   `forms/view.schema.json` (in which `spec` is unconstrained JSON). The json-render spec
   schema stays in `catalog.schema.json`, and each component's props are in `components`. If
   the owner meant the spec schema, swap one argument in `catalog-entry.ts`.
8. **`catalog.schema.json` changes form.** It is derived from Zod now, so it uses `anyOf`
   (branches are disjoint by `type` const), adds `propertyNames` and `"type": "string"` beside
   each `const`. The six assertions of `ui/tests/unit/catalog-schema.test.ts` were run against
   the derived schema while planning and hold.
9. **Committed outputs are stale on `cure/generator`, by instruction.** D never commits
   `api/`, `generated/cli/` or `ui/src/api/generated/`. So on this branch `gen-check` fails;
   `ui build` fails against the committed old-shape `api/mcp-apps.json` unless `OKF_MCP_APPS`
   points at scratch output; `ui/tests/unit/catalog-schema.test.ts` still reads the old
   committed schema. All three clear when the orchestrator regenerates at merge.
10. **`ui/src/features/views/catalog.ts` is not changed.** It was allowed "only to import"
    the shared module; the component list is read from `catalog.data.components` in
    `ui/scripts/catalog-response.ts`, so no import is needed there.
11. **`ui/scripts/*.ts` are in no tsconfig.** `app-declaration.ts` and `catalog-response.ts`
    are type-checked because tests import them (`tsconfig.tests.json`); `catalog-entry.ts` and
    `bundle-app.mjs` are only executed. `ui/tsconfig*.json` are not D's files.
12. **A 501 response is added to every operation.** Not in D's numbered requirements; it is
    the OpenAPI side of C's `ErrorCode::NotImplemented`, and without it the generated client
    types do not know the status.
13. **`generate` exceeds Clippy's 100-line limit between D.1 and D.2.** It already does at
    base (`:53-193`); D.2 splits it. The Clippy gate applies at the end of the package.
14. **Verification while planning.** The UI and script code in D.5–D.7 was executed in a
    scratch copy of `ui/` outside the repository at `b205c4a`: `tsc` clean, biome clean on the
    listed files, Vitest 47 tests in 10 files pass, `generate-catalog.mjs` wrote
    `catalog.json`, `bundle-app.mjs` built with a new-shape declaration and failed with the old
    one, and `openapi-ts` emitted `createSandboxCapabilityMutation` with one fewer query key.
    The Rust in D.1–D.5 was not compiled (the planner could not run cargo); the expected lock
    diff and the byte-identity of D.1 and D.2 output are predictions the steps verify.
