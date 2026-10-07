## Package H: HTTP binding

- **Branch:** `cure/http`
- **Worktree:** `D:\okf\cure\http` (run every command from this directory; cargo from PowerShell only)
- **Wave:** 3, after package E is merged into `integration/foundation-cure` (packages C, D, F, G are merged too).
- **Files allowed (exact):**
  - `crates/server/src/lib.rs`
  - `crates/server/tests/**` (only `crates/server/tests/bindings.rs` exists and is changed)
  - `crates/server/Cargo.toml` — `[dev-dependencies]` only (no change is expected: `tower` with `util` is already there; `tokio`, `axum`, `serde_json` are normal dependencies)
- **Must not touch:** every other file. In particular `crates/core/**` and `tests/support/**` — this package *reads* `crates/core/tests/support/{mod,counting}.rs`, `tests/support/check.rs` and `tests/support/application.rs` through `#[path]` and never edits them. Also not `xtask/**`, `api/`, `generated/`, `Cargo.toml`, `Cargo.lock`, `crates/server/AGENTS.md`.
- **SPEC / AGENTS sentences served:**
  - `crates/server/AGENTS.md`: "Existing JSON router expects a real Application and authenticated Principal. No guest-owner fallback and no trust granted for loopback."
  - SPEC §11: "Both produce the same internal Principal and pass through the same core authorization rules." and "An agent never receives the browser owner's session credential."
  - SPEC §8: "Typed wire errors expose AlreadyIssued, InProgress and draft/idempotency Conflict details the UI can act on." and "Human confirmation requires an explicit revision-bound browser action" (hence the session id must reach the handler).
  - `crates/contract/src/error.rs` module docs: "adapters map `conflict`, `already_issued` and `in_progress` to HTTP 409 with this body."
  - Design §3: "A generated skeleton returns a typed 'not implemented' error (HTTP 501), never a success."
  - SPEC §13: "raw HTTP/MCP clients alongside generated clients" — a raw client must get a typed JSON error for every failure, not axum's plain text.

### Conventions for every task in this package

- **Line numbers** are those of commit `b205c4a`. Package C has since edited `crates/server/src/lib.rs` only as far as it needed to keep it compiling (the 13-field macro matcher, and the arm `ErrorCode::NotImplemented => StatusCode::NOT_IMPLEMENTED`, its Tasks C.4 and C.3); package E did not touch it. Task H.1 replaces the whole file, so nothing depends on the exact lines.
- **Formatting:**

```powershell
rustfmt --edition 2024 crates\server\src\lib.rs crates\server\tests\bindings.rs
git status --short
```

  `rustfmt` on `bindings.rs` also visits the four files it includes by `#[path]`. They are already formatted; if `git status` shows any file outside `crates/server/`, run `git checkout -- <that file>` and report it — it is not yours to change.
- **Commit procedure:** write the task's message text to `$env:TEMP\cure-msg.txt` (UTF-8), then `git status --short` (only `crates/server/` files), `git add <the paths the task lists>`, `git commit -F $env:TEMP\cure-msg.txt`. In `Verified:` keep the command and replace a count only with the count you observed.
- **Test idiom** (shared interfaces): tests return `TestResult`, use `?`, `assert!`/`assert_eq!`, `err_of`, `some`. No `unwrap`, `expect`, `expect_err`, `panic!`, `unreachable!`, `[]` indexing, `#[allow]`, `#[expect]`.
- **Module item order** (`clippy.toml`): `use`, macros, types, `const`, `impl`, `fn`, then `mod` — so `macro_rules! bind_routes` comes before the structs, and the `#[path] mod …;` lines are last in the test file.
- **How the server tests include the shared fixtures** — exactly these three declarations, at the end of `crates/server/tests/bindings.rs`, at the crate root:

```rust
#[path = "../../../tests/support/application.rs"]
mod application;
#[path = "../../../tests/support/check.rs"]
mod check;
#[path = "../../core/tests/support/mod.rs"]
mod fixtures;
```

  `fixtures` is package E's `crates/core/tests/support/mod.rs`; its `pub mod counting;` resolves to `crates/core/tests/support/counting.rs` because a `#[path]`-loaded file owns its directory. The module **must** be reachable as `crate::check` (the fixtures' self-tests use it). `application` gives `FixtureApplication`; `fixtures` gives `FixturePorts`, `GrantTable`, `FixtureAccess`, `FixtureMutations`, `tenant`, `workspace`; `fixtures::counting` gives `CountingApplication`. The self-tests inside those files run again in this test binary; that is expected.

### Status and rejection mapping this package implements

| Cause | axum 0.8 default | This router |
| --- | --- | --- |
| No `Principal` extension | 500 `text/plain` "Missing request extension" (`axum-0.8.9/src/extract/rejection.rs:42-48`) | 401, `ApiError { code: unauthenticated }` |
| Body is not JSON syntax (`JsonSyntaxError`) | 400 `text/plain` | 400, `invalid_input`, message = the rejection's `body_text()` |
| JSON that cannot become the target type (`JsonDataError`; cannot occur for a `serde_json::Value` target, mapped for completeness) | 422 `text/plain` | 400, `invalid_input` |
| Missing or non-JSON `Content-Type` (`MissingJsonContentType`) | 415 `text/plain` | 400, `invalid_input` |
| Body over 2 MiB (`LengthLimitError`, `axum-core-0.5.6/src/extract/rejection.rs:40-48`) | 413 `text/plain` | 413, `too_large` |
| Any other body read failure (`UnknownBodyError`) | 400 `text/plain` | 400, `invalid_input` |
| `ErrorCode::NotImplemented` from dispatch | — | 501 |
| `ErrorCode::{Conflict, AlreadyIssued, InProgress, Cancelled}` | — | 409; with `ErrorDetail::InProgress { retry_after, .. }` also `Retry-After: <retry_after>` |
| other `ErrorCode`s | — | unchanged: `InvalidInput` 400, `Unauthenticated` 401, `Forbidden` 403, `NotFound` 404, `TooLarge` 413, `Unsupported` 422, `Unavailable` 503, `Internal` 500 |

Why 415 becomes 400: the HTTP status is derived from `ErrorCode` alone, and no `ErrorCode` maps to 415; the published contract declares only 400/401/403/404/409/413/422/500/503 (`xtask/src/api.rs:429`); 422 is bound to `unsupported`, whose meaning ("The requested representation or format is unsupported") is about what the server is asked to produce or convert, not the request envelope. A non-JSON envelope sent to a JSON-only API is invalid input.

**Note for the orchestrator (not a change in this package):** the router returns 501, and at `b205c4a` `xtask/src/api.rs:429` does not list `"501"` among the generated responses. The generator's response list needs `"501"` (package C's plan assigns it to package D), or generated clients will see an undeclared status.

### Task H.1: Bind the router to the new dispatch — `SessionId`, 401 for a missing principal, 501

**Files:**
- Modify (replace whole file): `crates/server/src/lib.rs` (80 lines at `b205c4a`)
- Modify (replace whole file): `crates/server/tests/bindings.rs`
- Test: `crates/server/tests/bindings.rs`

**Interfaces:**
- Consumes (package E):

```rust
pub struct Caller<'a> { pub principal: &'a Principal, pub session_id: Option<&'a str> }
pub struct DispatchPorts<'a> { pub access: &'a dyn AccessControl, pub mutations: &'a dyn MutationStore }
pub async fn dispatch(service: &dyn Application, ports: &DispatchPorts<'_>, caller: &Caller<'_>, operation_id: &str, input: serde_json::Value) -> Result<serde_json::Value, ApiError>;
```

  and, for tests, the fixtures listed in Conventions.
- Consumes (axum-core 0.5.6, `src/extract/mod.rs:53-63`, re-exported as `axum::extract::FromRequestParts`):

```rust
pub trait FromRequestParts<S>: Sized {
    /// If the extractor fails it'll use this "rejection" type. A rejection is
    /// a kind of error that can be converted into a response.
    type Rejection: IntoResponse;

    /// Perform the extraction.
    fn from_request_parts(
        parts: &mut Parts,
        state: &S,
    ) -> impl Future<Output = Result<Self, Self::Rejection>> + Send;
}
```

  axum implements it with `async fn` bodies that do not await (`axum-0.8.9/src/extension.rs:90`); this package does the same. `Parts` is `http::request::Parts` with `pub extensions: Extensions` (`http-1.5.0/src/request.rs:176`). In tests: `tower::ServiceExt::oneshot(self, req: Request) -> Oneshot<Self, Request>` (`tower-0.5.3/src/util/mod.rs:89`).
- Produces:

```rust
#[derive(Clone)]
pub struct BoundApplication {
    pub application: Arc<dyn Application>,
    pub access: Arc<dyn AccessControl>,
    pub mutations: Arc<dyn MutationStore>,       // the `effects` field is removed
}
/// Optional request extension for browser sessions; the server lane's middleware inserts it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionId(pub String);
pub fn router(bound: BoundApplication) -> Router;   // signature unchanged, see Deviations 1
```

  Private: `struct Authenticated { principal: Principal, session_id: Option<String> }` with `impl<S: Send + Sync> FromRequestParts<S> for Authenticated { type Rejection = Response; … }`, `async fn respond(…) -> Response`, `fn error_response(error: ApiError) -> Response`, `const JSON_BODY_LIMIT: usize = 2_097_152`.

- [ ] **Step 1: Confirm the branch point contains package E, and see the crate fail.**

```powershell
git log -1 --oneline
Select-String -Path crates\core\src\dispatch.rs -Pattern 'pub struct Caller'
Select-String -Path crates\core\tests\support\counting.rs -Pattern 'pub fn park_next', 'pub fn fail_once'
Select-String -Path tests\support\check.rs -Pattern 'pub fn err_of'
cargo test --locked -p okf-jawn-server --no-run 2>&1 | Select-String -Pattern '^error' | Select-Object -First 6
```

  Expected: the three searches match. The build fails in `crates/server/src/lib.rs` with `E0432` (unresolved import `okf_jawn_core::mutations::AbandonedEffects`) — that is the state this task cures. If a search does not match, stop: package E is not merged.

- [ ] **Step 2: Write the failing tests.** Replace `crates/server/tests/bindings.rs` with:

```rust
//! Binding tests: the real router, the shared test `Application` fixtures, and the core fixture
//! `AccessControl` and `MutationStore`. Identity middleware and storage are not involved; the
//! tests insert the `Principal` and `SessionId` extensions the middleware would.

use std::collections::BTreeMap;
use std::sync::Arc;

use axum::body::{Body, to_bytes};
use axum::http::{Request, StatusCode};
use axum::response::Response;
use axum::{Extension, Router};
use okf_jawn_contract::access::{AccessRoute, Permission, Principal};
use okf_jawn_contract::error::{ApiError, ErrorCode};
use okf_jawn_contract::identity::IdentityError;
use okf_jawn_core::ports::Application;
use okf_jawn_server::{BoundApplication, SessionId, router};
use serde_json::{Value, json};
use tower::ServiceExt;

use application::FixtureApplication;
use check::{TestResult, some};
use fixtures::counting::CountingApplication;
use fixtures::{FixturePorts, GrantTable, tenant, workspace};

const WORKSPACE: &str = "11111111-1111-4111-8111-111111111111";
const OTHER_WORKSPACE: &str = "22222222-2222-4222-8222-222222222222";
const REVISION: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const LIST_ITEMS: &str = "/api/items/list-items";

fn alice() -> Result<Principal, IdentityError> {
    Ok(Principal {
        subject: "alice".to_owned(),
        tenant_id: tenant("tenant-local")?,
        route: AccessRoute::BrowserSession,
        client_id: None,
        delegation: None,
    })
}

/// Alice may read and write one workspace and holds nothing on any other.
fn ports() -> Result<FixturePorts, serde_json::Error> {
    let mut table = GrantTable::default();
    table
        .workspaces
        .entry("alice".to_owned())
        .or_default()
        .insert(
            workspace(WORKSPACE)?,
            vec![Permission::Read, Permission::Write],
        );
    Ok(FixturePorts::new(table))
}

/// The real router over `application` and the fixture ports, with Alice signed in.
fn signed_in(
    application: Arc<dyn Application>,
    ports: &FixturePorts,
) -> Result<Router, Box<dyn std::error::Error>> {
    Ok(anonymous(application, ports).layer(Extension(alice()?)))
}

/// The real router with no identity middleware in front of it.
fn anonymous(application: Arc<dyn Application>, ports: &FixturePorts) -> Router {
    router(BoundApplication {
        application,
        access: ports.access.clone(),
        mutations: ports.mutations.clone(),
    })
}

fn post(
    path: &str,
    content_type: &str,
    body: impl Into<Body>,
) -> Result<Request<Body>, axum::http::Error> {
    Request::post(path)
        .header("content-type", content_type)
        .body(body.into())
}

fn post_json(path: &str, input: &Value) -> Result<Request<Body>, axum::http::Error> {
    post(path, "application/json", input.to_string())
}

async fn json_body(response: Response) -> Result<Value, Box<dyn std::error::Error>> {
    let content_type = response
        .headers()
        .get("content-type")
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned);
    if content_type.as_deref() != Some("application/json") {
        return Err(format!("expected a JSON body, got content type {content_type:?}").into());
    }
    let bytes = to_bytes(response.into_body(), 1_048_576).await?;
    Ok(serde_json::from_slice(&bytes)?)
}

async fn api_error(response: Response) -> Result<ApiError, Box<dyn std::error::Error>> {
    Ok(serde_json::from_value(json_body(response).await?)?)
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

#[tokio::test]
async fn a_read_succeeds_end_to_end_and_an_ungranted_workspace_is_403() -> TestResult {
    let ports = ports()?;
    let fixture = FixtureApplication {
        responses: BTreeMap::from([("list_items".to_owned(), listing())]),
    };
    let app = signed_in(Arc::new(fixture), &ports)?;

    let request = post_json(LIST_ITEMS, &list_items_body(WORKSPACE))?;
    let response = app.clone().oneshot(request).await?;
    assert_eq!(response.status(), StatusCode::OK);
    let body = json_body(response).await?;
    assert_eq!(body.get("items"), Some(&json!([])));
    assert_eq!(ports.access.lookups(), 1);

    let request = post_json(LIST_ITEMS, &list_items_body(OTHER_WORKSPACE))?;
    let response = app.oneshot(request).await?;
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    assert_eq!(api_error(response).await?.code, ErrorCode::Forbidden);
    Ok(())
}

#[tokio::test]
async fn a_missing_principal_is_401_with_an_api_error_body() -> TestResult {
    let ports = ports()?;
    let counting = Arc::new(CountingApplication::new());
    counting.set_response("list_items", listing())?;
    let app = anonymous(counting.clone(), &ports);
    let request = post_json(LIST_ITEMS, &list_items_body(WORKSPACE))?;
    let response = app.oneshot(request).await?;
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(api_error(response).await?.code, ErrorCode::Unauthenticated);
    assert_eq!(counting.call_count("list_items")?, 0);
    assert_eq!(ports.access.lookups(), 0);
    Ok(())
}

#[tokio::test]
async fn the_session_id_extension_reaches_the_handler_context() -> TestResult {
    let ports = ports()?;
    let counting = Arc::new(CountingApplication::new());
    counting.set_response("list_items", listing())?;
    let app = signed_in(counting.clone(), &ports)?;

    let in_session = app
        .clone()
        .layer(Extension(SessionId("session-1".to_owned())));
    let request = post_json(LIST_ITEMS, &list_items_body(WORKSPACE))?;
    assert_eq!(in_session.oneshot(request).await?.status(), StatusCode::OK);
    let request = post_json(LIST_ITEMS, &list_items_body(WORKSPACE))?;
    assert_eq!(app.oneshot(request).await?.status(), StatusCode::OK);

    let seen = counting.contexts("list_items")?;
    let with_session = some(seen.first(), "the call made inside a session")?;
    let without_session = some(seen.get(1), "the call made without a session")?;
    assert_eq!(with_session.session_id.as_deref(), Some("session-1"));
    assert_eq!(with_session.principal.subject, "alice");
    assert_eq!(without_session.session_id, None);
    Ok(())
}

#[tokio::test]
async fn a_not_implemented_handler_is_501() -> TestResult {
    let ports = ports()?;
    let counting = Arc::new(CountingApplication::new());
    counting.fail_once(
        "list_items",
        ApiError::new(ErrorCode::NotImplemented, "list_items is not built yet"),
    )?;
    let app = signed_in(counting.clone(), &ports)?;
    let request = post_json(LIST_ITEMS, &list_items_body(WORKSPACE))?;
    let response = app.oneshot(request).await?;
    assert_eq!(response.status(), StatusCode::NOT_IMPLEMENTED);
    let error = api_error(response).await?;
    assert_eq!(error.code, ErrorCode::NotImplemented);
    assert_eq!(error.message, "list_items is not built yet");
    Ok(())
}

#[path = "../../../tests/support/application.rs"]
mod application;
#[path = "../../../tests/support/check.rs"]
mod check;
#[path = "../../core/tests/support/mod.rs"]
mod fixtures;
```

- [ ] **Step 3: Run and watch it fail.**

```powershell
cargo test --locked -p okf-jawn-server --test bindings 2>&1 | Select-String -Pattern '^error' | Select-Object -First 8
```

  Expected: compile failure — `E0432` in `src/lib.rs` (`AbandonedEffects`), and in the test `E0432` for `okf_jawn_server::SessionId`.

- [ ] **Step 4: Implement.** Replace `crates/server/src/lib.rs` with:

```rust
//! HTTP binding for the declared application port.
//!
//! Every canonical operation is one JSON `POST` route that calls `core::dispatch`. Identity
//! middleware, written by the server lane, inserts the authenticated `Principal` and, for
//! browser sessions, a `SessionId` as request extensions before this router runs. A request
//! that reaches a route without a `Principal` is answered 401; there is no anonymous owner
//! fallback. A dispatch failure is an `ApiError` JSON body whose status follows its `ErrorCode`.

use std::sync::Arc;

use axum::extract::{DefaultBodyLimit, FromRequestParts, State};
use axum::http::StatusCode;
use axum::http::request::Parts;
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use axum::{Json, Router};
use okf_jawn_contract::access::Principal;
use okf_jawn_contract::error::{ApiError, ErrorCode};
use okf_jawn_core::access::AccessControl;
use okf_jawn_core::dispatch::{Caller, DispatchPorts, dispatch};
use okf_jawn_core::mutations::MutationStore;
use okf_jawn_core::ports::Application;
use serde_json::Value;

macro_rules! bind_routes {
    ($(($id:ident, $request:ty, $response:ty, $path:literal, $label:literal, $alias:literal,
        $operator:literal, $visibility:literal, $permission:ident, $ui:literal, $status:literal,
        $destructive:literal, $description:literal)),* $(,)?) => {
        /// Bind every canonical operation to its shared domain implementation.
        ///
        /// A request without an authenticated `Principal` extension is answered 401.
        pub fn router(bound: BoundApplication) -> Router {
            let router = Router::new();
            $(let router = router.route($path, post(
                |State(bound): State<BoundApplication>,
                 authenticated: Authenticated,
                 Json(input): Json<Value>| async move {
                    respond(&bound, &authenticated, input, stringify!($id), $status).await
                },
            ));)*
            router.layer(DefaultBodyLimit::max(JSON_BODY_LIMIT)).with_state(bound)
        }
    };
}

/// Application handlers plus the ports dispatch needs for authorization and idempotency.
#[derive(Clone)]
pub struct BoundApplication {
    /// Typed operation handlers.
    pub application: Arc<dyn Application>,
    /// Workspace and tenant grant resolution.
    pub access: Arc<dyn AccessControl>,
    /// Idempotency ledger.
    pub mutations: Arc<dyn MutationStore>,
}

/// Browser session identity. Session middleware inserts it as a request extension; bearer and
/// connector callers have none.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionId(pub String);

/// The authenticated caller taken from request extensions.
struct Authenticated {
    principal: Principal,
    session_id: Option<String>,
}

/// Largest JSON request body accepted, in bytes (2 MiB).
const JSON_BODY_LIMIT: usize = 2_097_152;

impl<S: Send + Sync> FromRequestParts<S> for Authenticated {
    type Rejection = Response;

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        let Some(principal) = parts.extensions.get::<Principal>().cloned() else {
            return Err(error_response(ApiError::new(
                ErrorCode::Unauthenticated,
                "Authentication is required",
            )));
        };
        let session_id = parts
            .extensions
            .get::<SessionId>()
            .map(|session| session.0.clone());
        Ok(Self {
            principal,
            session_id,
        })
    }
}

/// Run one operation for an authenticated caller and render its outcome.
async fn respond(
    bound: &BoundApplication,
    authenticated: &Authenticated,
    input: Value,
    operation_id: &str,
    success_status: u16,
) -> Response {
    let ports = DispatchPorts {
        access: bound.access.as_ref(),
        mutations: bound.mutations.as_ref(),
    };
    let caller = Caller {
        principal: &authenticated.principal,
        session_id: authenticated.session_id.as_deref(),
    };
    match dispatch(
        bound.application.as_ref(),
        &ports,
        &caller,
        operation_id,
        input,
    )
    .await
    {
        Ok(output) => match StatusCode::from_u16(success_status) {
            Ok(status) => (status, Json(output)).into_response(),
            Err(_) => error_response(ApiError::new(
                ErrorCode::Internal,
                "Invalid declared response status",
            )),
        },
        Err(error) => error_response(error),
    }
}

fn error_response(error: ApiError) -> Response {
    let status = match error.code {
        ErrorCode::InvalidInput => StatusCode::BAD_REQUEST,
        ErrorCode::Unauthenticated => StatusCode::UNAUTHORIZED,
        ErrorCode::Forbidden => StatusCode::FORBIDDEN,
        ErrorCode::NotFound => StatusCode::NOT_FOUND,
        ErrorCode::Conflict
        | ErrorCode::AlreadyIssued
        | ErrorCode::InProgress
        | ErrorCode::Cancelled => StatusCode::CONFLICT,
        ErrorCode::TooLarge => StatusCode::PAYLOAD_TOO_LARGE,
        ErrorCode::Unsupported => StatusCode::UNPROCESSABLE_ENTITY,
        ErrorCode::NotImplemented => StatusCode::NOT_IMPLEMENTED,
        ErrorCode::Unavailable => StatusCode::SERVICE_UNAVAILABLE,
        ErrorCode::Internal => StatusCode::INTERNAL_SERVER_ERROR,
    };
    (status, Json(error)).into_response()
}

okf_jawn_contract::for_each_operation!(bind_routes);
```

  Keep the macro's 13-field matcher identical to the tuple in the shared interfaces. The stale doc sentence "Missing authenticated `Principal` extensions reject requests" (line 38) is replaced by the 401 statement above.

- [ ] **Step 5: Run and watch it pass.**

```powershell
cargo test --locked -p okf-jawn-server 2>&1 | Select-String -Pattern '^test result|FAILED|^error'
```

  Expected: `bindings` reports `12 passed; 0 failed` (4 binding tests, 4 `check::tests`, 2 + 2 fixture self-tests).

- [ ] **Step 6: Format and commit.**

```powershell
git add crates/server/src/lib.rs crates/server/tests/bindings.rs
```

```text
fix(server): bind the router to the new dispatch and answer 401 for a missing principal.

Why: Package E changed dispatch to take Caller { principal, session_id } and removed AbandonedEffects, so crates/server no longer compiled. Review finding: a request without a Principal extension got axum's 500 text/plain "Missing request extension" instead of a typed 401 (server AGENTS: no guest-owner fallback).
What changed: BoundApplication loses effects; a private Authenticated extractor reads the Principal and the optional SessionId extension and rejects with a 401 ApiError body; the session id reaches OperationContext; not_implemented maps to 501; the module docs no longer say "reject". Tests use the real router with the shared FixtureApplication and the core fixture AccessControl and MutationStore.
Verified: cargo test --locked -p okf-jawn-server -> bindings 12 passed, 0 failed.
Next: Task H.2, body and content-type rejections as ApiError.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_01LoZr4ME3crLYjcTNn5GcAC
```

### Task H.2: Malformed, mistyped and oversized bodies are `ApiError` JSON

**Files:**
- Modify: `crates/server/src/lib.rs` — module docs, imports, the route closure in `bind_routes`, `respond`, new `body_rejection`
- Modify: `crates/server/tests/bindings.rs`
- Test: `crates/server/tests/bindings.rs`

**Interfaces:**
- Consumes (axum 0.8.9 / axum-core 0.5.6):

```rust
// axum-core-0.5.6/src/extract/mod.rs:119-129 — lets a handler receive the rejection instead of axum answering
impl<S, T> FromRequest<S> for Result<T, T::Rejection>
where
    T: FromRequest<S>,
    S: Send + Sync,
{
    type Rejection = Infallible;
    async fn from_request(req: Request, state: &S) -> Result<Self, Self::Rejection> {
        Ok(T::from_request(req, state).await)
    }
}

// axum-0.8.9/src/extract/rejection.rs:126-139 — #[non_exhaustive]
pub enum JsonRejection { JsonDataError, JsonSyntaxError, MissingJsonContentType, BytesRejection }
// generated for it by axum-core-0.5.6/src/macros.rs:182-201
pub fn body_text(&self) -> String;
pub fn status(&self) -> http::StatusCode;
```

  `Json::from_request` checks the content type first, then buffers through `Bytes::from_request`, which applies the `DefaultBodyLimit` with `http_body_util::Limited` (`axum-0.8.9/src/json.rs:106-113`, `axum-core-0.5.6/src/ext_traits/request.rs:316-328`); an over-limit body surfaces as `LengthLimitError`, whose `status()` is 413.
- Produces: private `fn body_rejection(rejection: &JsonRejection) -> ApiError`; `respond` takes `body: Result<Json<Value>, JsonRejection>`. Because `JsonRejection` is `#[non_exhaustive]`, the mapping keys on `status()`: 413 → `too_large`, everything else → `invalid_input`. The principal is still checked first, so an unauthenticated caller learns nothing about its body.

- [ ] **Step 1: Write the failing tests.** In `crates/server/tests/bindings.rs`, add below the `LIST_ITEMS` constant:

```rust
/// One byte more than the router's 2 MiB JSON body limit.
const OVER_LIMIT: usize = 2_097_153;
```

  and insert above the `#[path = …] mod application;` line at the end of the file:

```rust
#[tokio::test]
async fn malformed_json_is_400_invalid_input() -> TestResult {
    let ports = ports()?;
    let counting = Arc::new(CountingApplication::new());
    let app = signed_in(counting.clone(), &ports)?;
    let request = post(LIST_ITEMS, "application/json", "{not json")?;
    let response = app.oneshot(request).await?;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let body = json_body(response).await?;
    assert_eq!(body.get("code"), Some(&json!("invalid_input")));
    assert_eq!(counting.call_count("list_items")?, 0);
    Ok(())
}

#[tokio::test]
async fn a_wrong_content_type_is_400_invalid_input() -> TestResult {
    let ports = ports()?;
    let counting = Arc::new(CountingApplication::new());
    counting.set_response("list_items", listing())?;
    let app = signed_in(counting.clone(), &ports)?;
    let request = post(
        LIST_ITEMS,
        "text/plain",
        list_items_body(WORKSPACE).to_string(),
    )?;
    let response = app.oneshot(request).await?;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let error = api_error(response).await?;
    assert_eq!(error.code, ErrorCode::InvalidInput);
    assert!(error.message.contains("Content-Type"));
    assert_eq!(counting.call_count("list_items")?, 0);
    Ok(())
}

#[tokio::test]
async fn a_body_over_the_limit_is_413_too_large() -> TestResult {
    let ports = ports()?;
    let counting = Arc::new(CountingApplication::new());
    let app = signed_in(counting.clone(), &ports)?;
    let request = post(LIST_ITEMS, "application/json", vec![b' '; OVER_LIMIT])?;
    let response = app.oneshot(request).await?;
    assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);
    let body = json_body(response).await?;
    assert_eq!(body.get("code"), Some(&json!("too_large")));
    assert_eq!(counting.call_count("list_items")?, 0);
    Ok(())
}
```

- [ ] **Step 2: Run and watch them fail.**

```powershell
cargo test --locked -p okf-jawn-server --test bindings 2>&1 | Select-String -Pattern 'FAILED|^Error|left:|right:|^test result'
```

  Expected: three failures. `malformed_json_is_400_invalid_input` and `a_body_over_the_limit_is_413_too_large`: `Error: "expected a JSON body, got content type Some(\"text/plain; charset=utf-8\")"`. `a_wrong_content_type_is_400_invalid_input`: status assertion, `left: 415`, `right: 400`.

- [ ] **Step 3: Implement.** Replace the `//!` header with:

```rust
//! HTTP binding for the declared application port.
//!
//! Every canonical operation is one JSON `POST` route that calls `core::dispatch`. Identity
//! middleware, written by the server lane, inserts the authenticated `Principal` and, for
//! browser sessions, a `SessionId` as request extensions before this router runs. A request
//! that reaches a route without a `Principal` is answered 401; there is no anonymous owner
//! fallback. Every failure, including a malformed, mistyped or oversized body, is an `ApiError`
//! JSON body whose status follows its `ErrorCode`.
```

  Add `use axum::extract::rejection::JsonRejection;` as the first `axum` import. Replace `macro_rules! bind_routes { … }` with:

```rust
macro_rules! bind_routes {
    ($(($id:ident, $request:ty, $response:ty, $path:literal, $label:literal, $alias:literal,
        $operator:literal, $visibility:literal, $permission:ident, $ui:literal, $status:literal,
        $destructive:literal, $description:literal)),* $(,)?) => {
        /// Bind every canonical operation to its shared domain implementation.
        ///
        /// A request without an authenticated `Principal` extension is answered 401.
        pub fn router(bound: BoundApplication) -> Router {
            let router = Router::new();
            $(let router = router.route($path, post(
                |State(bound): State<BoundApplication>,
                 authenticated: Authenticated,
                 body: Result<Json<Value>, JsonRejection>| async move {
                    respond(&bound, &authenticated, body, stringify!($id), $status).await
                },
            ));)*
            router.layer(DefaultBodyLimit::max(JSON_BODY_LIMIT)).with_state(bound)
        }
    };
}
```

  Replace `async fn respond` with, and add `fn body_rejection` directly below it:

```rust
/// Run one operation for an authenticated caller and render its outcome.
async fn respond(
    bound: &BoundApplication,
    authenticated: &Authenticated,
    body: Result<Json<Value>, JsonRejection>,
    operation_id: &str,
    success_status: u16,
) -> Response {
    let input = match body {
        Ok(Json(input)) => input,
        Err(rejection) => return error_response(body_rejection(&rejection)),
    };
    let ports = DispatchPorts {
        access: bound.access.as_ref(),
        mutations: bound.mutations.as_ref(),
    };
    let caller = Caller {
        principal: &authenticated.principal,
        session_id: authenticated.session_id.as_deref(),
    };
    match dispatch(
        bound.application.as_ref(),
        &ports,
        &caller,
        operation_id,
        input,
    )
    .await
    {
        Ok(output) => match StatusCode::from_u16(success_status) {
            Ok(status) => (status, Json(output)).into_response(),
            Err(_) => error_response(ApiError::new(
                ErrorCode::Internal,
                "Invalid declared response status",
            )),
        },
        Err(error) => error_response(error),
    }
}

/// Describe a body that could not be read as JSON.
///
/// An over-limit body is `too_large` (413). Everything else, including a missing or wrong
/// `Content-Type` (which axum would answer 415), is `invalid_input` (400): the API accepts only
/// JSON, and 415 is not among the statuses the published contract declares.
fn body_rejection(rejection: &JsonRejection) -> ApiError {
    if rejection.status() == StatusCode::PAYLOAD_TOO_LARGE {
        ApiError::new(ErrorCode::TooLarge, "Request body exceeds the 2 MiB limit")
    } else {
        ApiError::new(ErrorCode::InvalidInput, rejection.body_text())
    }
}
```

- [ ] **Step 4: Run and watch them pass.**

```powershell
cargo test --locked -p okf-jawn-server 2>&1 | Select-String -Pattern '^test result|FAILED|^error'
```

  Expected: `bindings` reports `15 passed; 0 failed`.

- [ ] **Step 5: Format and commit.**

```powershell
git add crates/server/src/lib.rs crates/server/tests/bindings.rs
```

```text
fix(server): answer malformed, mistyped and oversized bodies with ApiError JSON.

Why: SPEC section 13 requires raw HTTP clients alongside generated ones; axum's own rejections were text/plain (400, 415, 413), which no client of the declared contract can parse, and 415 is not a declared status (xtask/src/api.rs:429).
What changed: the route takes Result<Json<Value>, JsonRejection>; body_rejection maps an over-limit body to too_large (413) and every other body or content-type failure to invalid_input (400), keeping axum's message text.
Verified: cargo test --locked -p okf-jawn-server -> bindings 15 passed, 0 failed; the three new tests failed before the change (text/plain body, status 415).
Next: Task H.3, Retry-After on in_progress.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_01LoZr4ME3crLYjcTNn5GcAC
```

### Task H.3: `Retry-After` on an in-progress refusal

**Files:**
- Modify: `crates/server/src/lib.rs` — imports, `error_response`, new `retry_after`
- Modify: `crates/server/tests/bindings.rs`
- Test: `crates/server/tests/bindings.rs`

**Interfaces:**
- Consumes: `ApiError.detail: Option<Box<ErrorDetail>>`, `ErrorDetail::InProgress { mutation_id: MutationId, retry_after: u32 }` (contract); `impl From<u32> for HeaderValue` (`http-1.5.0/src/header/value.rs:427-433`); `http::header::RETRY_AFTER`; fixture `CountingApplication::{park_next, entered, resume}`.
- Produces: private `fn retry_after(error: &ApiError) -> Option<HeaderValue>`; `error_response` adds `Retry-After` whenever the error carries an `InProgress` detail. The test produces the in-progress state through the router: one request is parked inside its handler (it holds the lease), a second identical request is refused.

- [ ] **Step 1: Write the failing test.** In `crates/server/tests/bindings.rs`: change the import to `use okf_jawn_contract::error::{ApiError, ErrorCode, ErrorDetail};`; add below `LIST_ITEMS`:

```rust
const CREATE_ITEM: &str = "/api/items/create-item";
```

  add after `fn listing`:

```rust
fn create_item_body() -> Value {
    json!({
        "workspace_id": WORKSPACE,
        "base_revision": REVISION,
        "path": "notes/a.md",
        "title": "a",
        "type_name": "note",
        "kind": "note",
        "body": "hello",
        "properties": {},
        "idempotency_key": "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa"
    })
}

fn item_document() -> Value {
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
        "body": "hello",
        "properties": {}
    })
}
```

  and insert above the `mod application;` lines:

```rust
#[tokio::test]
async fn in_progress_is_409_with_retry_after_from_the_detail() -> TestResult {
    let ports = ports()?;
    let counting = Arc::new(CountingApplication::new());
    counting.set_response("create_item", item_document())?;
    counting.park_next("create_item")?;
    let app = signed_in(counting.clone(), &ports)?;

    let running = app
        .clone()
        .oneshot(post_json(CREATE_ITEM, &create_item_body())?);
    tokio::pin!(running);
    tokio::select! {
        biased;
        outcome = &mut running => {
            let status = outcome.map(|response| response.status());
            return Err(format!("the first request finished while parked: {status:?}").into());
        }
        () = counting.entered() => {}
    }

    let response = app
        .oneshot(post_json(CREATE_ITEM, &create_item_body())?)
        .await?;
    assert_eq!(response.status(), StatusCode::CONFLICT);
    let header = some(
        response
            .headers()
            .get("retry-after")
            .and_then(|value| value.to_str().ok())
            .map(str::to_owned),
        "the Retry-After header",
    )?;
    let error = api_error(response).await?;
    assert_eq!(error.code, ErrorCode::InProgress);
    let detail = some(error.detail, "the in-progress detail")?;
    let ErrorDetail::InProgress { retry_after, .. } = *detail else {
        return Err("expected an in-progress detail".into());
    };
    assert!(retry_after >= 1);
    assert_eq!(header, retry_after.to_string());

    counting.resume();
    assert_eq!(running.await?.status(), StatusCode::OK);
    assert_eq!(counting.call_count("create_item")?, 1);
    Ok(())
}
```

- [ ] **Step 2: Run and watch it fail.**

```powershell
cargo test --locked -p okf-jawn-server --test bindings in_progress_is_409 2>&1 | Select-String -Pattern 'FAILED|^Error'
```

  Expected: `FAILED` with `Error: "expected the Retry-After header to be present"`.

- [ ] **Step 3: Implement.** Make the import block read exactly:

```rust
use std::sync::Arc;

use axum::extract::rejection::JsonRejection;
use axum::extract::{DefaultBodyLimit, FromRequestParts, State};
use axum::http::header::RETRY_AFTER;
use axum::http::request::Parts;
use axum::http::{HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use axum::{Json, Router};
use okf_jawn_contract::access::Principal;
use okf_jawn_contract::error::{ApiError, ErrorCode, ErrorDetail};
use okf_jawn_core::access::AccessControl;
use okf_jawn_core::dispatch::{Caller, DispatchPorts, dispatch};
use okf_jawn_core::mutations::MutationStore;
use okf_jawn_core::ports::Application;
use serde_json::Value;
```

  Replace `fn error_response` with, and add `fn retry_after` directly below it:

```rust
fn error_response(error: ApiError) -> Response {
    let status = match error.code {
        ErrorCode::InvalidInput => StatusCode::BAD_REQUEST,
        ErrorCode::Unauthenticated => StatusCode::UNAUTHORIZED,
        ErrorCode::Forbidden => StatusCode::FORBIDDEN,
        ErrorCode::NotFound => StatusCode::NOT_FOUND,
        ErrorCode::Conflict
        | ErrorCode::AlreadyIssued
        | ErrorCode::InProgress
        | ErrorCode::Cancelled => StatusCode::CONFLICT,
        ErrorCode::TooLarge => StatusCode::PAYLOAD_TOO_LARGE,
        ErrorCode::Unsupported => StatusCode::UNPROCESSABLE_ENTITY,
        ErrorCode::NotImplemented => StatusCode::NOT_IMPLEMENTED,
        ErrorCode::Unavailable => StatusCode::SERVICE_UNAVAILABLE,
        ErrorCode::Internal => StatusCode::INTERNAL_SERVER_ERROR,
    };
    let wait = retry_after(&error);
    let mut response = (status, Json(error)).into_response();
    if let Some(value) = wait {
        response.headers_mut().insert(RETRY_AFTER, value);
    }
    response
}

/// The `Retry-After` value for an in-progress refusal, taken from its typed detail.
fn retry_after(error: &ApiError) -> Option<HeaderValue> {
    let ErrorDetail::InProgress {
        retry_after: seconds,
        ..
    } = error.detail.as_deref()?
    else {
        return None;
    };
    Some(HeaderValue::from(*seconds))
}
```

- [ ] **Step 4: Run and watch it pass.**

```powershell
cargo test --locked -p okf-jawn-server 2>&1 | Select-String -Pattern '^test result|FAILED|^error'
```

  Expected: `bindings` reports `16 passed; 0 failed`.

- [ ] **Step 5: Format and commit.**

```powershell
git add crates/server/src/lib.rs crates/server/tests/bindings.rs
```

```text
feat(server): send Retry-After with an in-progress refusal.

Why: SPEC section 8: typed wire errors expose InProgress details the UI can act on; the detail's retry_after was only in the body, so a plain HTTP client had nothing to honour.
What changed: error_response adds Retry-After: <retry_after> whenever the ApiError carries ErrorDetail::InProgress; the status stays 409.
Verified: cargo test --locked -p okf-jawn-server -> bindings 16 passed, 0 failed; in_progress_is_409_with_retry_after_from_the_detail failed on the missing header before the change.
Next: Task H.4, the package gate.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_01LoZr4ME3crLYjcTNn5GcAC
```

### Task H.4: Package gate and handoff

**Files:**
- Modify: `crates/server/src/lib.rs`, `crates/server/tests/bindings.rs` — only if the gate reports a diagnostic in them
- Test: the package gate

**Interfaces:**
- Consumes / Produces: none.

- [ ] **Step 1: Check the final shape of `crates/server/src/lib.rs`.** Top-level items in this order: imports; `macro_rules! bind_routes`; `BoundApplication`; `SessionId`; `Authenticated`; `const JSON_BODY_LIMIT`; `impl FromRequestParts for Authenticated`; `respond`; `body_rejection`; `error_response`; `retry_after`; `okf_jawn_contract::for_each_operation!(bind_routes);`. No `AbandonedEffects`, no `Extension` import.

- [ ] **Step 2: Run the gate, unfiltered.**

```powershell
cargo fmt --all --check
cargo clippy --locked -p okf-jawn-server --all-targets -- -D warnings
cargo test --locked -p okf-jawn-server
```

  Expected: all three exit 0; the test run ends with `bindings` `16 passed; 0 failed`. A diagnostic in `crates/server/` is your defect: fix it without `#[allow]`/`#[expect]` (rename, restructure, backtick an identifier in a doc comment) and re-run. A diagnostic in any other file is not yours: stop and report file, line and output.

- [ ] **Step 3: Commit, only if Step 2 required a change.**

```powershell
git add crates/server/src/lib.rs crates/server/tests/bindings.rs
```

```text
refactor(server): clear the lint findings of the package gate.

Why: The package gate (fmt, Clippy with -D warnings on all targets, tests) must pass with no filter before the binding merges.
What changed: no behaviour change; the edits are exactly the ones the gate named, without any suppression attribute.
Verified: cargo fmt --all --check -> exit 0; cargo clippy --locked -p okf-jawn-server --all-targets -- -D warnings -> exit 0; cargo test --locked -p okf-jawn-server -> bindings 16 passed, 0 failed.
Next: Package I (integrate): regenerate, add 501 to the generated response list, run premerge.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_01LoZr4ME3crLYjcTNn5GcAC
```

- [ ] **Step 4: Handoff.** Report the commits, the three gate results as observed, and the two notes for the orchestrator: the missing `"501"` in `xtask/src/api.rs:429`, and Deviations 1.

### Package H acceptance

A verifying agent that did not write the package runs, from `D:\okf\cure\http` in PowerShell:

1. Scope: `git diff --name-only (git merge-base HEAD integration/foundation-cure) HEAD` lists exactly `crates/server/src/lib.rs` and `crates/server/tests/bindings.rs`.
2. `cargo fmt --all --check` → exit 0.
3. `cargo clippy --locked -p okf-jawn-server --all-targets -- -D warnings` → exit 0.
4. `cargo test --locked -p okf-jawn-server` → `bindings`: 16 passed, 0 failed.
5. Text searches (each must print nothing):

```powershell
Select-String -Path crates\server\tests\*.rs -Pattern '\.unwrap\(\)', '\.expect\(', 'expect_err', 'panic!', 'unreachable!', '#\[allow', '#\[expect', '\w\["'
Select-String -Path crates\server\src\lib.rs, crates\server\tests\bindings.rs -Pattern 'AbandonedEffects', 'NoopMutations', 'PermitAllAccess', 'impl MutationStore', 'impl AccessControl'
Select-String -Path crates\server\src\lib.rs -Pattern 'tests/support', 'FixtureApplication'
```

6. Mutation checks. Apply one edit to `crates/server/src/lib.rs`, run `cargo test --locked -p okf-jawn-server --test bindings`, see the named tests fail, then `git checkout -- crates/server/src/lib.rs`.

   | Edit | Tests that must fail |
   | --- | --- |
   | `from_request_parts`: `ErrorCode::Unauthenticated` → `ErrorCode::Internal` | `a_missing_principal_is_401_with_an_api_error_body` |
   | `respond`: `session_id: authenticated.session_id.as_deref(),` → `session_id: None,` | `the_session_id_extension_reaches_the_handler_context` |
   | `error_response`: `ErrorCode::NotImplemented => StatusCode::NOT_IMPLEMENTED,` → `ErrorCode::NotImplemented => StatusCode::INTERNAL_SERVER_ERROR,` | `a_not_implemented_handler_is_501` |
   | `bind_routes`: `stringify!($id)` → `"get_health"` | `a_read_succeeds_end_to_end_and_an_ungranted_workspace_is_403`, `the_session_id_extension_reaches_the_handler_context` |
   | `body_rejection`: replace the whole body with `ApiError::new(ErrorCode::Internal, rejection.body_text())` | `malformed_json_is_400_invalid_input`, `a_wrong_content_type_is_400_invalid_input`, `a_body_over_the_limit_is_413_too_large` |
   | `body_rejection`: `rejection.status() == StatusCode::PAYLOAD_TOO_LARGE` → `false` | `a_body_over_the_limit_is_413_too_large` |
   | `bind_routes`: `DefaultBodyLimit::max(JSON_BODY_LIMIT)` → `DefaultBodyLimit::disable()` | `a_body_over_the_limit_is_413_too_large` (the oversized body is then read and refused as malformed, 400) |
   | `error_response`: delete the `if let Some(value) = wait { … }` block | `in_progress_is_409_with_retry_after_from_the_detail` |
   | `error_response`: move `ErrorCode::InProgress` from the 409 arm to the `ErrorCode::Unavailable` arm | `in_progress_is_409_with_retry_after_from_the_detail` |

### Deviations

1. **`router` keeps its current signature.** The brief proposes `pub fn router(service: Arc<dyn Application>, access: Arc<dyn AccessControl>, mutations: Arc<dyn MutationStore>) -> Router` and says to confirm against the current one and adapt minimally. The current one is `pub fn router(bound: BoundApplication) -> Router` (`crates/server/src/lib.rs:39`), with `BoundApplication` also serving as the axum state. The minimal adaptation is to drop the `effects` field and keep the function signature; that is what this plan does. If the three-argument form is wanted, it is a one-line wrapper and a decision for the orchestrator.
2. **415 and 422 become 400.** See the mapping table: a missing or wrong `Content-Type` (axum 415) and a JSON data error (axum 422) are both `invalid_input` / 400, because the status is derived from `ErrorCode`, 415 is not a declared status, and 422 means `unsupported`.
3. **501 is returned but may not be declared.** At `b205c4a`, `xtask/src/api.rs:429` lists "400", "401", "403", "404", "409", "413", "422", "500", "503". Package C's Deviations 9 assigns the `501` entry to package D. Not changed here (outside this package's files); the orchestrator should confirm it is present in `api/openapi` after D merges.
4. **The `NotImplemented` arm predates this package.** `error_response` matches `ErrorCode` exhaustively, so package C's Task C.3 added `ErrorCode::NotImplemented => StatusCode::NOT_IMPLEMENTED` to keep the crate compiling, without a test (C's Deviations 9). `a_not_implemented_handler_is_501` is that missing test: it guards the mapping rather than driving it, and the acceptance mutation applies.
5. **Task H.1's four tests fail first by compile error only.** After package E the server crate does not compile at all, so nothing can be shown failing at run time before Task H.1; each of those tests has a mutation in the acceptance table instead. Tasks H.2 and H.3 fail first at run time.
6. **The in-progress test does not use a stub store.** It uses the fixture `MutationStore` and real concurrency through the router (a parked handler holds the lease), and asserts that the header equals the `retry_after` in the body's detail rather than a literal, because the fixture's lease is measured on the wall clock.
7. **Lane gate with `--features runtime` is not part of this package's gate.** The brief fixes the gate to `-p okf-jawn-server` without features; `crates/server/AGENTS.md` and CI's `runtime-clippy` job use `--features runtime`. Nothing in this package is feature-dependent; the orchestrator's `premerge` (`--all-features`) covers it.
8. **Shared fixtures are compiled into this test crate.** Their self-tests run here too (12 of the 16 tests are binding tests plus `check` and fixture self-tests), and any lint in them is reported by this package's Clippy run although the files belong to package E. Such a finding is a stop-and-report, not an edit.
9. **Not compiled.** No cargo run was permitted while writing this plan. Both listings of `lib.rs` and of `bindings.rs` were formatted with the repository's `rustfmt.toml`; the axum, axum-core, http and tower APIs were read from the vendored sources cited in each task; nothing was type-checked. One known contingency: if Clippy reports `unused_async` on `from_request_parts` (not expected — it is a trait method, and axum writes its own impls the same way), write it as a plain `fn` returning `std::future::ready(…)` of the same `Result`; do not suppress the lint.
