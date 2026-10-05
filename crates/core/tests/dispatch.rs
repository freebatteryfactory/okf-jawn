//! A5 dispatch tests: grants, targets, delegation ceiling, and mutation ledger outcomes.

mod support;

use std::collections::BTreeMap;
use std::error::Error;
use std::sync::Mutex;

use okf_jawn_contract::access::{AccessRoute, DelegationCeiling, Permission, Principal};
use okf_jawn_contract::error::ErrorCode;
use okf_jawn_contract::identity::{ConfirmationId, MutationId};
use okf_jawn_contract::metadata::OperationName;
use okf_jawn_contract::review::{ConfirmationAction, ConfirmationTarget};
use okf_jawn_core::confirmations::{ConfirmationConsume, ConfirmationCreate, ConfirmationStore};
use okf_jawn_core::dispatch::{DispatchPorts, dispatch};
use okf_jawn_core::mutations::MutationStore;
use okf_jawn_core::ports::PortFuture;
use okf_jawn_core::storage::StorageScope;
use serde_json::{Value, json};
use support::counting::CountingApplication;
use support::{FixturePorts, GrantTable, idempotency_key, tenant, workspace};
use uuid::Uuid;

fn principal(subject: &str, route: AccessRoute) -> Principal {
    Principal {
        subject: subject.to_owned(),
        tenant_id: tenant("tenant-local"),
        route,
        client_id: None,
        delegation: None,
    }
}

fn all_perms() -> Vec<Permission> {
    vec![
        Permission::Read,
        Permission::Write,
        Permission::Propose,
        Permission::Approve,
        Permission::Review,
        Permission::Admin,
    ]
}

fn ports_admin_a_read_b() -> FixturePorts {
    let a = workspace("11111111-1111-4111-8111-111111111111");
    let b = workspace("22222222-2222-4222-8222-222222222222");
    let mut workspaces = BTreeMap::new();
    workspaces.insert(
        a,
        vec![
            Permission::Read,
            Permission::Write,
            Permission::Propose,
            Permission::Approve,
            Permission::Review,
            Permission::Admin,
        ],
    );
    workspaces.insert(b, vec![Permission::Read]);
    let mut subjects = BTreeMap::new();
    subjects.insert("alice".to_owned(), workspaces);
    let mut tenants = BTreeMap::new();
    tenants.insert("alice".to_owned(), all_perms());
    FixturePorts::new(GrantTable {
        workspaces: subjects,
        tenants,
    })
}

fn dispatch_ports(ports: &FixturePorts) -> DispatchPorts<'_> {
    DispatchPorts {
        access: ports.access.as_ref(),
        mutations: ports.mutations.as_ref(),
        effects: ports.effects.as_ref(),
    }
}

fn write_item_body(workspace_id: &str, key: &str) -> Value {
    json!({
        "workspace_id": workspace_id,
        "base_revision": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        "path": "notes/a.md",
        "title": "a",
        "type_name": "note",
        "kind": "note",
        "body": "hello",
        "properties": {},
        "idempotency_key": key
    })
}

fn item_document(body: &str) -> Value {
    json!({
        "summary": {
            "id": "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa",
            "path": "notes/a.md",
            "title": "a",
            "description": "",
            "type_name": "note",
            "kind": "note",
            "revision": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            "lifecycle": "active"
        },
        "body": body,
        "properties": {}
    })
}

#[tokio::test]
async fn admin_in_a_write_succeeds_write_in_b_forbidden_read_in_b_ok() -> Result<(), Box<dyn Error>>
{
    let ports = ports_admin_a_read_b();
    let app = CountingApplication::new();
    app.set_response("create_item", item_document("hello"))?;
    app.set_response(
        "list_items",
        json!({
            "revision": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            "items": [],
            "folders": []
        }),
    )?;
    let alice = principal("alice", AccessRoute::LocalOwner);
    let dp = dispatch_ports(&ports);
    let key = "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa";
    let ok = dispatch(
        &app,
        &dp,
        &alice,
        "create_item",
        write_item_body("11111111-1111-4111-8111-111111111111", key),
    )
    .await?;
    assert!(ok.get("body").is_some());
    assert_eq!(app.call_count("create_item")?, 1);

    let forbidden = dispatch(
        &app,
        &dp,
        &alice,
        "create_item",
        write_item_body(
            "22222222-2222-4222-8222-222222222222",
            "bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb",
        ),
    )
    .await
    .expect_err("write in B must fail");
    assert_eq!(forbidden.code, ErrorCode::Forbidden);
    assert_eq!(app.call_count("create_item")?, 1);

    let read = dispatch(
        &app,
        &dp,
        &alice,
        "list_items",
        json!({
            "workspace_id": "22222222-2222-4222-8222-222222222222",
            "at": {"kind": "latest"},
            "folder": "",
            "page": {"limit": 10}
        }),
    )
    .await?;
    assert!(read.get("items").is_some());
    assert_eq!(app.call_count("list_items")?, 1);
    Ok(())
}

#[tokio::test]
async fn present_with_foreign_binding_is_forbidden_before_handler() -> Result<(), Box<dyn Error>> {
    let ports = ports_admin_a_read_b();
    let app = CountingApplication::new();
    app.set_response(
        "present_view",
        json!({
            "view": {
                "schema_version": 1,
                "title": "t",
                "description": "d",
                "mode": "pinned",
                "grammar": "json_render",
                "bindings": [],
                "spec": {},
                "charts": {}
            },
            "resolved_bindings": [],
            "warnings": [],
            "receipt_id": "eeeeeeee-eeee-4eee-8eee-eeeeeeeeeeee"
        }),
    )?;
    let alice = principal("alice", AccessRoute::LocalOwner);
    let err = dispatch(
        &app,
        &dispatch_ports(&ports),
        &alice,
        "present_view",
        json!({
            "workspace_id": "11111111-1111-4111-8111-111111111111",
            "view": {
                "schema_version": 1,
                "title": "t",
                "description": "d",
                "mode": "pinned",
                "grammar": "json_render",
                "bindings": [{
                    "name": "src",
                    "source": {
                        "workspace_id": "33333333-3333-4333-8333-333333333333",
                        "item_id": "cccccccc-cccc-4ccc-8ccc-cccccccccccc",
                        "path": "notes/a.md",
                        "revision": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
                        "selection": {"kind": "all"}
                    },
                    "units": {},
                    "transforms": []
                }],
                "spec": {},
                "charts": {}
            }
        }),
    )
    .await
    .expect_err("foreign binding must fail");
    assert_eq!(err.code, ErrorCode::Forbidden);
    assert_eq!(app.call_count("present_view")?, 0);
    Ok(())
}

#[tokio::test]
async fn create_workspace_without_tenant_grant_is_forbidden() -> Result<(), Box<dyn Error>> {
    let a = workspace("11111111-1111-4111-8111-111111111111");
    let mut workspaces = BTreeMap::new();
    workspaces.insert(a, all_perms());
    let mut subjects = BTreeMap::new();
    subjects.insert("alice".to_owned(), workspaces);
    let ports = FixturePorts::new(GrantTable {
        workspaces: subjects,
        tenants: BTreeMap::new(),
    });
    let app = CountingApplication::new();
    app.set_response(
        "create_workspace",
        json!({
            "id": "11111111-1111-4111-8111-111111111111",
            "name": "n",
            "description": "d",
            "head": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            "created_at": "2026-01-01T00:00:00Z",
            "permissions": ["admin"]
        }),
    )?;
    let err = dispatch(
        &app,
        &dispatch_ports(&ports),
        &principal("alice", AccessRoute::LocalOwner),
        "create_workspace",
        json!({
            "name": "n",
            "description": "d",
            "idempotency_key": "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa"
        }),
    )
    .await
    .expect_err("missing tenant grant");
    assert_eq!(err.code, ErrorCode::Forbidden);
    assert_eq!(app.call_count("create_workspace")?, 0);
    Ok(())
}

#[tokio::test]
async fn delegation_ceiling_blocks_write_for_read_only_connector() -> Result<(), Box<dyn Error>> {
    let ports = ports_admin_a_read_b();
    let app = CountingApplication::new();
    app.set_response("create_item", item_document("hello"))?;
    let mut alice = principal("alice", AccessRoute::McpDelegation);
    alice.client_id = Some("connector".to_owned());
    alice.delegation = Some(DelegationCeiling {
        permissions: vec![Permission::Read],
        workspace_ids: None,
    });
    let err = dispatch(
        &app,
        &dispatch_ports(&ports),
        &alice,
        "create_item",
        write_item_body(
            "11111111-1111-4111-8111-111111111111",
            "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa",
        ),
    )
    .await
    .expect_err("delegated read cannot write");
    assert_eq!(err.code, ErrorCode::Forbidden);
    assert_eq!(app.call_count("create_item")?, 0);
    Ok(())
}

#[tokio::test]
async fn replay_returns_identical_response_and_handler_runs_once() -> Result<(), Box<dyn Error>> {
    let ports = ports_admin_a_read_b();
    let app = CountingApplication::new();
    app.set_response("create_item", item_document("hello"))?;
    let alice = principal("alice", AccessRoute::LocalOwner);
    let body = write_item_body(
        "11111111-1111-4111-8111-111111111111",
        "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa",
    );
    let first = dispatch(&app, &dispatch_ports(&ports), &alice, "create_item", body.clone()).await?;
    let second = dispatch(&app, &dispatch_ports(&ports), &alice, "create_item", body).await?;
    assert_eq!(first, second);
    assert_eq!(app.call_count("create_item")?, 1);
    Ok(())
}

#[tokio::test]
async fn conflict_when_key_reused_with_different_body() -> Result<(), Box<dyn Error>> {
    let ports = ports_admin_a_read_b();
    let app = CountingApplication::new();
    app.set_response("create_item", item_document("hello"))?;
    let alice = principal("alice", AccessRoute::LocalOwner);
    let key = "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa";
    let _ = dispatch(
        &app,
        &dispatch_ports(&ports),
        &alice,
        "create_item",
        write_item_body("11111111-1111-4111-8111-111111111111", key),
    )
    .await?;
    let mut other = write_item_body("11111111-1111-4111-8111-111111111111", key);
    other["body"] = json!("different");
    let err = dispatch(&app, &dispatch_ports(&ports), &alice, "create_item", other)
        .await
        .expect_err("digest conflict");
    assert_eq!(err.code, ErrorCode::Conflict);
    assert_eq!(app.call_count("create_item")?, 1);
    Ok(())
}

#[tokio::test]
async fn in_progress_when_second_begin_during_live_lease() -> Result<(), Box<dyn Error>> {
    let ports = ports_admin_a_read_b();
    let key = idempotency_key("aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa");
    let mutation_key = okf_jawn_core::mutations::MutationKey {
        tenant_id: tenant("tenant-local"),
        subject: "alice".to_owned(),
        operation: OperationName::CreateItem,
        key,
    };
    let digest = okf_jawn_core::mutations::request_digest(&write_item_body(
        "11111111-1111-4111-8111-111111111111",
        "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa",
    ))?;
    let first = ports.mutations.begin(&mutation_key, &digest).await?;
    assert!(matches!(
        first,
        okf_jawn_core::mutations::BeginOutcome::New(_)
    ));
    let second = ports.mutations.begin(&mutation_key, &digest).await?;
    assert!(matches!(
        second,
        okf_jawn_core::mutations::BeginOutcome::InProgress { .. }
    ));
    Ok(())
}

#[tokio::test]
async fn abandoned_git_reconciles_via_effects_without_second_handler() -> Result<(), Box<dyn Error>>
{
    let ports = ports_admin_a_read_b();
    let app = CountingApplication::new();
    let key = "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa";
    let body = write_item_body("11111111-1111-4111-8111-111111111111", key);
    let mutation_key = okf_jawn_core::mutations::MutationKey {
        tenant_id: tenant("tenant-local"),
        subject: "alice".to_owned(),
        operation: OperationName::CreateItem,
        key: idempotency_key(key),
    };
    let digest = okf_jawn_core::mutations::request_digest(&body)?;
    let outcome = ports.mutations.begin(&mutation_key, &digest).await?;
    let okf_jawn_core::mutations::BeginOutcome::New(mutation_id) = outcome else {
        panic!("expected New");
    };
    let stored = item_document("hello");
    ports
        .effects
        .insert(OperationName::CreateItem, mutation_id, stored.clone())?;
    ports.mutations.force_abandon(&mutation_key)?;

    let alice = principal("alice", AccessRoute::LocalOwner);
    let response = dispatch(&app, &dispatch_ports(&ports), &alice, "create_item", body).await?;
    assert_eq!(response, stored);
    assert_eq!(app.call_count("create_item")?, 0);
    Ok(())
}

#[tokio::test]
async fn abandoned_connector_reconciles_without_second_secret() -> Result<(), Box<dyn Error>> {
    let mut tenants = BTreeMap::new();
    tenants.insert("alice".to_owned(), all_perms());
    let a = workspace("11111111-1111-4111-8111-111111111111");
    let mut workspaces = BTreeMap::new();
    workspaces.insert(a, all_perms());
    let mut subjects = BTreeMap::new();
    subjects.insert("alice".to_owned(), workspaces);
    let ports = FixturePorts::new(GrantTable {
        workspaces: subjects,
        tenants,
    });
    let app = CountingApplication::new();
    let key = "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa";
    let body = json!({
        "label": "agent",
        "workspace_ids": ["11111111-1111-4111-8111-111111111111"],
        "allow_propose": false,
        "idempotency_key": key
    });
    let mutation_key = okf_jawn_core::mutations::MutationKey {
        tenant_id: tenant("tenant-local"),
        subject: "alice".to_owned(),
        operation: OperationName::CreateConnector,
        key: idempotency_key(key),
    };
    let digest = okf_jawn_core::mutations::request_digest(&body)?;
    let outcome = ports.mutations.begin(&mutation_key, &digest).await?;
    let okf_jawn_core::mutations::BeginOutcome::New(mutation_id) = outcome else {
        panic!("expected New");
    };
    let connector_id = "cccccccc-cccc-4ccc-8ccc-cccccccccccc";
    ports.effects.insert(
        OperationName::CreateConnector,
        mutation_id,
        json!({ "connector_id": connector_id }),
    )?;
    ports.mutations.force_abandon(&mutation_key)?;

    let err = dispatch(
        &app,
        &dispatch_ports(&ports),
        &principal("alice", AccessRoute::LocalOwner),
        "create_connector",
        body,
    )
    .await
    .expect_err("already issued");
    assert_eq!(err.code, ErrorCode::AlreadyIssued);
    assert_eq!(app.call_count("create_connector")?, 0);
    let detail = err.detail.expect("AlreadyIssued detail");
    let okf_jawn_contract::error::ErrorDetail::AlreadyIssued {
        connector_id: found,
    } = *detail
    else {
        panic!("expected AlreadyIssued detail");
    };
    assert_eq!(found.0.to_string(), connector_id);
    Ok(())
}

#[tokio::test]
async fn cross_subject_keys_are_independent() -> Result<(), Box<dyn Error>> {
    let a = workspace("11111111-1111-4111-8111-111111111111");
    let mut map = BTreeMap::new();
    map.insert(a, all_perms());
    let mut subjects = BTreeMap::new();
    subjects.insert("x".to_owned(), map.clone());
    subjects.insert("y".to_owned(), map);
    let mut tenants = BTreeMap::new();
    tenants.insert("x".to_owned(), all_perms());
    tenants.insert("y".to_owned(), all_perms());
    let ports = FixturePorts::new(GrantTable {
        workspaces: subjects,
        tenants,
    });
    let app = CountingApplication::new();
    app.set_response("create_item", item_document("from-x"))?;
    let key = "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa";
    let body = write_item_body("11111111-1111-4111-8111-111111111111", key);
    let x = dispatch(
        &app,
        &dispatch_ports(&ports),
        &principal("x", AccessRoute::LocalOwner),
        "create_item",
        body.clone(),
    )
    .await?;
    app.set_response("create_item", item_document("from-y"))?;
    let y = dispatch(
        &app,
        &dispatch_ports(&ports),
        &principal("y", AccessRoute::LocalOwner),
        "create_item",
        body,
    )
    .await?;
    assert_ne!(x.get("body"), y.get("body"));
    assert_eq!(app.call_count("create_item")?, 2);
    Ok(())
}

#[tokio::test]
async fn create_connector_replay_is_already_issued_without_secret() -> Result<(), Box<dyn Error>> {
    let mut tenants = BTreeMap::new();
    tenants.insert("alice".to_owned(), all_perms());
    let a = workspace("11111111-1111-4111-8111-111111111111");
    let mut workspaces = BTreeMap::new();
    workspaces.insert(a, all_perms());
    let mut subjects = BTreeMap::new();
    subjects.insert("alice".to_owned(), workspaces);
    let ports = FixturePorts::new(GrantTable {
        workspaces: subjects,
        tenants,
    });
    let app = CountingApplication::new();
    app.set_response(
        "create_connector",
        json!({
            "connector": {
                "connector_id": "cccccccc-cccc-4ccc-8ccc-cccccccccccc",
                "label": "agent",
                "workspace_ids": ["11111111-1111-4111-8111-111111111111"],
                "permissions": ["read"],
                "created_at": "2026-01-01T00:00:00Z"
            },
            "secret": "super-secret-value"
        }),
    )?;
    let body = json!({
        "label": "agent",
        "workspace_ids": ["11111111-1111-4111-8111-111111111111"],
        "allow_propose": false,
        "idempotency_key": "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa"
    });
    let alice = principal("alice", AccessRoute::LocalOwner);
    let first = dispatch(
        &app,
        &dispatch_ports(&ports),
        &alice,
        "create_connector",
        body.clone(),
    )
    .await?;
    assert_eq!(first["secret"], "super-secret-value");
    let err = dispatch(&app, &dispatch_ports(&ports), &alice, "create_connector", body)
        .await
        .expect_err("replay must be AlreadyIssued");
    assert_eq!(err.code, ErrorCode::AlreadyIssued);
    assert_eq!(app.call_count("create_connector")?, 1);
    let stored = ports
        .mutations
        .find(app.last_mutation.lock().unwrap().expect("mutation"))
        .await?
        .expect("ledger row");
    assert!(stored.body.get("secret").is_none());
    assert!(stored.body.get("connector_id").is_some());
    Ok(())
}

#[tokio::test]
async fn confirmation_reconsume_same_mutation_succeeds_different_fails()
-> Result<(), Box<dyn Error>> {
    let store = FixtureConfirmationStore::default();
    let scope = StorageScope {
        tenant_id: tenant("tenant-local"),
        workspace_id: workspace("11111111-1111-4111-8111-111111111111"),
    };
    let create_id = MutationId(Uuid::parse_str("aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa")?);
    let confirmation = store
        .create(
            &scope,
            create_id,
            ConfirmationCreate {
                action: ConfirmationAction::AcceptProposal,
                target: ConfirmationTarget::Proposal {
                    proposal_id: serde_json::from_str(
                        "\"bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb\"",
                    )?,
                },
                revision: "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".parse()?,
                content_digest:
                    "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".parse()?,
                session_id: "session".to_owned(),
                subject: "alice".to_owned(),
                expires_at: "2099-01-01T00:00:00Z".to_owned(),
            },
        )
        .await?;
    let consume = ConfirmationConsume {
        confirmation_id: confirmation.id,
        action: ConfirmationAction::AcceptProposal,
        target: ConfirmationTarget::Proposal {
            proposal_id: serde_json::from_str("\"bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb\"")?,
        },
        revision: "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".parse()?,
        content_digest: "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
            .parse()?,
        session_id: "session".to_owned(),
        subject: "alice".to_owned(),
    };
    let m1 = MutationId(Uuid::parse_str("cccccccc-cccc-4ccc-8ccc-cccccccccccc")?);
    let m2 = MutationId(Uuid::parse_str("dddddddd-dddd-4ddd-8ddd-dddddddddddd")?);
    store.consume(&scope, m1, consume.clone()).await?;
    // Crash before complete: retry with same MutationId succeeds.
    store.consume(&scope, m1, consume.clone()).await?;
    let err = store
        .consume(&scope, m2, consume)
        .await
        .expect_err("different mutation cannot re-consume");
    assert_eq!(err.code, ErrorCode::Conflict);
    Ok(())
}

#[tokio::test]
async fn write_without_idempotency_key_rejected_by_schema() -> Result<(), Box<dyn Error>> {
    let ports = ports_admin_a_read_b();
    let app = CountingApplication::new();
    let err = dispatch(
        &app,
        &dispatch_ports(&ports),
        &principal("alice", AccessRoute::LocalOwner),
        "create_item",
        json!({
            "workspace_id": "11111111-1111-4111-8111-111111111111",
            "base_revision": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            "path": "notes/a.md",
            "title": "a",
            "type_name": "note",
            "kind": "note",
            "body": "hello",
            "properties": {}
        }),
    )
    .await
    .expect_err("missing idempotency_key");
    assert_eq!(err.code, ErrorCode::InvalidInput);
    assert_eq!(app.call_count("create_item")?, 0);
    Ok(())
}

/// In-memory ConfirmationStore proving MutationId-scoped re-consume.
#[derive(Default)]
struct FixtureConfirmationStore {
    rows: Mutex<BTreeMap<ConfirmationId, ConfirmationRow>>,
}

struct ConfirmationRow {
    confirmation: okf_jawn_contract::review::Confirmation,
    create: ConfirmationCreate,
    consumed_by: Option<MutationId>,
}

impl ConfirmationStore for FixtureConfirmationStore {
    fn create<'a>(
        &'a self,
        _scope: &'a StorageScope,
        _mutation_id: MutationId,
        create: ConfirmationCreate,
    ) -> PortFuture<'a, okf_jawn_contract::review::Confirmation> {
        Box::pin(async move {
            let confirmation = okf_jawn_contract::review::Confirmation {
                id: ConfirmationId(Uuid::new_v4()),
                expires_at: create.expires_at.clone(),
                revision: create.revision.clone(),
            };
            self.rows
                .lock()
                .map_err(|_| {
                    okf_jawn_contract::error::ApiError::new(ErrorCode::Internal, "lock poisoned")
                })?
                .insert(
                    confirmation.id,
                    ConfirmationRow {
                        confirmation: confirmation.clone(),
                        create,
                        consumed_by: None,
                    },
                );
            Ok(confirmation)
        })
    }

    fn find_by_mutation<'a>(
        &'a self,
        _scope: &'a StorageScope,
        _mutation_id: MutationId,
    ) -> PortFuture<'a, Option<okf_jawn_contract::review::Confirmation>> {
        Box::pin(async move { Ok(None) })
    }

    fn consume<'a>(
        &'a self,
        _scope: &'a StorageScope,
        mutation_id: MutationId,
        consume: ConfirmationConsume,
    ) -> PortFuture<'a, okf_jawn_contract::review::Confirmation> {
        Box::pin(async move {
            let mut rows = self.rows.lock().map_err(|_| {
                okf_jawn_contract::error::ApiError::new(ErrorCode::Internal, "lock poisoned")
            })?;
            let row = rows.get_mut(&consume.confirmation_id).ok_or_else(|| {
                okf_jawn_contract::error::ApiError::new(ErrorCode::NotFound, "confirmation missing")
            })?;
            if row.create.action != consume.action
                || row.create.target != consume.target
                || row.create.revision != consume.revision
                || row.create.content_digest != consume.content_digest
                || row.create.session_id != consume.session_id
                || row.create.subject != consume.subject
            {
                return Err(okf_jawn_contract::error::ApiError::new(
                    ErrorCode::Forbidden,
                    "confirmation binding mismatch",
                ));
            }
            match row.consumed_by {
                None => {
                    row.consumed_by = Some(mutation_id);
                    Ok(row.confirmation.clone())
                }
                Some(prior) if prior == mutation_id => Ok(row.confirmation.clone()),
                Some(_) => Err(okf_jawn_contract::error::ApiError::new(
                    ErrorCode::Conflict,
                    "confirmation already used by another mutation",
                )),
            }
        })
    }
}
