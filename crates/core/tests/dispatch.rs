//! Dispatch tests: every case goes through `dispatch` with the fixture `AccessControl` and the
//! fixture `MutationStore`, and fails when the rule it names is removed.

use std::future::Future;
use std::pin::Pin;

use okf_jawn_contract::access::{AccessRoute, DelegationCeiling, Permission, Principal};
use okf_jawn_contract::error::{ApiError, ErrorCode, ErrorDetail};
use okf_jawn_contract::identity::IdentityError;
use okf_jawn_core::context::Attempt;
use okf_jawn_core::dispatch::{Caller, DispatchPorts, dispatch};
use okf_jawn_core::mutations::request_digest;
use okf_jawn_core::storage::StorageScope;
use serde_json::{Value, json};

use check::{TestResult, err_of, some};
use support::counting::CountingApplication;
use support::{FixturePorts, GrantTable, LedgerCall, all_permissions, tenant, workspace};

/// A dispatch that was started and is still running, held so a test can finish or drop it.
type Running<'a> = Pin<Box<dyn Future<Output = Result<Value, ApiError>> + 'a>>;

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
///
/// The dispatch future is boxed: it holds one state per declared operation, and awaiting it
/// inline would make every test future that large (`clippy::large_futures`).
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
    Box::pin(dispatch(
        app,
        &dispatch_ports(ports),
        &caller,
        operation,
        input,
    ))
    .await
}

/// Start `operation` and wait until its handler is parked. The returned attempt holds its lease
/// until the test resumes the handler and awaits it, or drops it.
async fn park_in_handler<'a>(
    app: &'a CountingApplication,
    ports: &'a FixturePorts,
    principal: &'a Principal,
    operation: &'a str,
    input: Value,
) -> Result<Running<'a>, Box<dyn std::error::Error>> {
    app.park_next(operation)?;
    let mut attempt: Running<'a> = Box::pin(call(app, ports, principal, operation, input));
    tokio::select! {
        biased;
        outcome = &mut attempt => Err(Box::<dyn std::error::Error>::from(format!(
            "the attempt finished while its handler was parked: {outcome:?}"
        ))),
        () = app.entered() => Ok(attempt),
    }
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
    drop(park_in_handler(app, ports, principal, operation, input).await?);
    Ok(())
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
async fn a_tenant_grant_without_admin_cannot_create_a_workspace() -> TestResult {
    let mut table = GrantTable::default();
    table.tenants.insert(
        "bob".to_owned(),
        vec![
            Permission::Read,
            Permission::Write,
            Permission::Propose,
            Permission::Approve,
            Permission::Review,
        ],
    );
    let ports = FixturePorts::new(table);
    let app = CountingApplication::new();
    app.set_response("create_workspace", created_workspace())?;
    let bob = principal("bob", AccessRoute::BrowserSession)?;
    let input = create_workspace_body(KEY_ONE);
    let refused = err_of(call(&app, &ports, &bob, "create_workspace", input).await)?;
    assert_eq!(refused.code, ErrorCode::Forbidden);
    // The adapter was asked and answered with Bob's grant: the refusal is the product's.
    assert_eq!(ports.access.lookups(), 1);
    assert_eq!(app.call_count("create_workspace")?, 0);
    Ok(())
}

#[tokio::test]
async fn a_connector_of_a_tenant_admin_cannot_create_a_workspace() -> TestResult {
    let ports = ports_admin_a_read_b()?;
    let app = CountingApplication::new();
    app.set_response("create_workspace", created_workspace())?;
    // Alice's tenant grant and the connector's ceiling both include Admin, so the route rule is
    // the only one left to refuse.
    let agent = connector("alice", all_permissions())?;
    let input = create_workspace_body(KEY_ONE);
    let refused = err_of(call(&app, &ports, &agent, "create_workspace", input).await)?;
    assert_eq!(refused.code, ErrorCode::Forbidden);
    assert_eq!(ports.access.lookups(), 1);
    assert_eq!(app.call_count("create_workspace")?, 0);
    Ok(())
}

#[tokio::test]
async fn a_delegation_ceiling_decides_whether_a_tenant_admin_grant_applies() -> TestResult {
    let ports = ports_admin_a_read_b()?;
    let app = CountingApplication::new();
    app.set_response("create_workspace", created_workspace())?;

    // A service route may administer, and Alice's tenant grant includes Admin, so the ceiling
    // is the only rule left to refuse.
    let mut narrowed = principal("alice", AccessRoute::Service)?;
    narrowed.delegation = Some(DelegationCeiling {
        permissions: vec![Permission::Read, Permission::Write],
        workspace_ids: None,
    });
    let input = create_workspace_body(KEY_ONE);
    let refused = err_of(call(&app, &ports, &narrowed, "create_workspace", input).await)?;
    assert_eq!(refused.code, ErrorCode::Forbidden);
    assert_eq!(ports.access.lookups(), 1);
    assert_eq!(app.call_count("create_workspace")?, 0);

    let mut administering = principal("alice", AccessRoute::Service)?;
    administering.delegation = Some(DelegationCeiling {
        permissions: vec![Permission::Read, Permission::Admin],
        workspace_ids: None,
    });
    let input = create_workspace_body(KEY_TWO);
    let created = call(&app, &ports, &administering, "create_workspace", input).await?;
    assert_eq!(created.get("id"), Some(&json!(WORKSPACE_A)));
    let seen = app.contexts("create_workspace")?;
    let context = some(seen.first(), "the handler context")?;
    let tenant_grant = some(context.tenant.as_ref(), "the tenant grant")?;
    // The handler sees the grant already narrowed to the ceiling.
    assert_eq!(
        tenant_grant.permissions,
        vec![Permission::Read, Permission::Admin]
    );
    Ok(())
}

#[tokio::test]
async fn a_tenant_admin_creates_a_workspace() -> TestResult {
    let ports = ports_admin_a_read_b()?;
    let app = CountingApplication::new();
    app.set_response("create_workspace", created_workspace())?;
    let alice = principal("alice", AccessRoute::LocalOwner)?;
    let input = create_workspace_body(KEY_ONE);
    let created = call(&app, &ports, &alice, "create_workspace", input).await?;
    assert_eq!(created.get("id"), Some(&json!(WORKSPACE_A)));
    assert_eq!(app.call_count("create_workspace")?, 1);
    let seen = app.contexts("create_workspace")?;
    let context = some(seen.first(), "the handler context")?;
    let tenant_grant = some(context.tenant.as_ref(), "the tenant grant")?;
    assert_eq!(tenant_grant.tenant_id, alice.tenant_id);
    assert!(tenant_grant.allows(Permission::Admin));
    assert_eq!(context.grants, Vec::new());
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
async fn a_slow_attempt_whose_lease_was_taken_over_cannot_complete() -> TestResult {
    let ports = ports_admin_a_read_b()?;
    let app = CountingApplication::new();
    app.set_response("create_item", item_document("from the second attempt"))?;
    let alice = principal("alice", AccessRoute::LocalOwner)?;
    let body = create_item_body(WORKSPACE_A, KEY_ONE, "hello");
    let slow = park_in_handler(&app, &ports, &alice, "create_item", body.clone()).await?;
    ports.mutations.expire_leases()?;

    let second = call(&app, &ports, &alice, "create_item", body.clone()).await?;
    assert_eq!(second.get("body"), Some(&json!("from the second attempt")));
    let seen = app.contexts("create_item")?;
    let slow_context = some(seen.first(), "the slow attempt")?;
    let second_context = some(seen.get(1), "the attempt that took the lease over")?;
    let mutation_id = some(slow_context.mutation, "the mutation id")?;
    assert_eq!(second_context.mutation, Some(mutation_id));
    assert_eq!(second_context.attempt, Attempt::Resumed);

    // The slow handler now returns, with another response, under a lease it no longer holds.
    app.set_response("create_item", item_document("from the slow attempt"))?;
    app.resume();
    let refused = err_of(slow.await)?;
    assert_eq!(refused.code, ErrorCode::Conflict);
    assert_eq!(
        ports.mutations.stored_body(mutation_id)?,
        Some(second.clone())
    );
    let replayed = call(&app, &ports, &alice, "create_item", body).await?;
    assert_eq!(replayed, second);
    assert_eq!(app.call_count("create_item")?, 2);
    Ok(())
}

#[tokio::test]
async fn a_failed_attempt_whose_lease_was_taken_over_leaves_the_new_lease_held() -> TestResult {
    let ports = ports_admin_a_read_b()?;
    let app = CountingApplication::new();
    app.set_response("create_item", item_document("hello"))?;
    let alice = principal("alice", AccessRoute::LocalOwner)?;
    let body = create_item_body(WORKSPACE_A, KEY_ONE, "hello");
    let slow = park_in_handler(&app, &ports, &alice, "create_item", body.clone()).await?;
    ports.mutations.expire_leases()?;
    let current = park_in_handler(&app, &ports, &alice, "create_item", body.clone()).await?;

    // Parked handlers resume oldest first, so the failure goes to the slow attempt.
    app.fail_once(
        "create_item",
        ApiError::new(ErrorCode::Unavailable, "index offline"),
    )?;
    app.resume();
    let failed = err_of(slow.await)?;
    assert_eq!(failed.code, ErrorCode::Unavailable);

    // Its release named a lease that is no longer the grant, so the mutation is still held.
    let refused = err_of(call(&app, &ports, &alice, "create_item", body).await)?;
    assert_eq!(refused.code, ErrorCode::InProgress);
    assert_eq!(app.call_count("create_item")?, 2);

    app.resume();
    let finished = current.await?;
    assert_eq!(finished.get("body"), Some(&json!("hello")));
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
    assert_eq!(context.grants, Vec::new());
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
    Box::pin(dispatch(
        &app,
        &dispatch_ports(&ports),
        &in_session,
        "list_items",
        list_items_body(WORKSPACE_A),
    ))
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

#[tokio::test]
async fn a_failed_begin_is_returned_and_the_handler_does_not_run() -> TestResult {
    let ports = ports_admin_a_read_b()?;
    let app = CountingApplication::new();
    app.set_response("create_item", item_document("hello"))?;
    ports.mutations.fail_next(
        LedgerCall::Begin,
        ApiError::new(ErrorCode::Unavailable, "ledger offline"),
    )?;
    let alice = principal("alice", AccessRoute::LocalOwner)?;
    let body = create_item_body(WORKSPACE_A, KEY_ONE, "hello");
    let refused = err_of(call(&app, &ports, &alice, "create_item", body).await)?;
    assert_eq!(refused.code, ErrorCode::Unavailable);
    assert_eq!(refused.message, "ledger offline");
    assert_eq!(app.call_count("create_item")?, 0);
    Ok(())
}

#[tokio::test]
async fn a_failed_complete_releases_the_lease_so_a_retry_resumes_at_once() -> TestResult {
    let ports = ports_admin_a_read_b()?;
    let app = CountingApplication::new();
    app.set_response("create_item", item_document("hello"))?;
    ports.mutations.fail_next(
        LedgerCall::Complete,
        ApiError::new(ErrorCode::Unavailable, "ledger offline"),
    )?;
    let alice = principal("alice", AccessRoute::LocalOwner)?;
    let body = create_item_body(WORKSPACE_A, KEY_ONE, "hello");
    let refused = err_of(call(&app, &ports, &alice, "create_item", body.clone()).await)?;
    assert_eq!(refused.code, ErrorCode::Unavailable);
    assert_eq!(refused.message, "ledger offline");

    // The lease was released, not left to expire: the retry is not `InProgress`.
    let response = call(&app, &ports, &alice, "create_item", body).await?;
    assert_eq!(response.get("body"), Some(&json!("hello")));
    let seen = app.contexts("create_item")?;
    assert_eq!(seen.len(), 2);
    let earlier = some(seen.first(), "the attempt whose complete failed")?;
    let later = some(seen.get(1), "the retry")?;
    assert!(earlier.mutation.is_some());
    assert_eq!(later.mutation, earlier.mutation);
    assert_eq!(later.attempt, Attempt::Resumed);
    Ok(())
}

#[tokio::test]
async fn a_failed_release_does_not_replace_the_handler_error() -> TestResult {
    let ports = ports_admin_a_read_b()?;
    let app = CountingApplication::new();
    app.fail_once(
        "create_item",
        ApiError::new(ErrorCode::Unavailable, "index offline"),
    )?;
    ports.mutations.fail_next(
        LedgerCall::Release,
        ApiError::new(ErrorCode::Internal, "ledger offline"),
    )?;
    let alice = principal("alice", AccessRoute::LocalOwner)?;
    let body = create_item_body(WORKSPACE_A, KEY_ONE, "hello");
    let refused = err_of(call(&app, &ports, &alice, "create_item", body).await)?;
    assert_eq!(refused.code, ErrorCode::Unavailable);
    assert_eq!(refused.message, "index offline");
    assert_eq!(app.call_count("create_item")?, 1);
    Ok(())
}

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

#[path = "../../../tests/support/check.rs"]
mod check;
mod support;
