//! Dispatch's two rules of core M1.4: a mutation of an archived workspace that
//! `operations::ALLOWED_WHILE_ARCHIVED` does not list is refused before its handler and before
//! the ledger (contract ruling 25), and the connector operations run only for the local owner
//! (`operations::LOCAL_OWNER_ONLY`, contract ruling 22), with a refused `create_connector`
//! naming the listed workspace that refused it. Every case goes through `dispatch` with the
//! fixture grants and ledger, and fails when the rule it names is removed.

use std::collections::BTreeSet;

use okf_jawn_contract::access::{AccessRoute, Permission, Principal};
use okf_jawn_contract::error::{ApiError, ErrorCode, ErrorDetail};
use okf_jawn_contract::identity::{IdentityError, Timestamp};
use okf_jawn_contract::metadata::OperationName;
use okf_jawn_contract::operations::{ALLOWED_WHILE_ARCHIVED, LOCAL_OWNER_ONLY};
use okf_jawn_core::access::check_local_owner_route;
use okf_jawn_core::context::Attempt;
use okf_jawn_core::dispatch::{Caller, DispatchPorts, dispatch};
use okf_jawn_core::events::EventScope;
use serde_json::{Value, json};

use archiving::Archiving;
use check::{TestResult, err_of, some};
use support::{FixturePorts, GrantTable, all_permissions, tenant, workspace};

macro_rules! mutation_ids {
    ($(($id:ident, $request:ty, $response:ty, $path:literal, $label:literal, $alias:literal,
        $operator:literal, $visibility:literal, $permission:ident, $ui:literal, $status:literal,
        $destructive:literal, $description:literal)),* $(,)?) => {
        /// Every operation whose request carries an idempotency key: the mutations.
        fn mutations() -> Result<BTreeSet<&'static str>, serde_json::Error> {
            let mut found = BTreeSet::new();
            $(
                let schema = serde_json::to_value(schemars::schema_for!($request))?;
                if schema.pointer("/properties/idempotency_key").is_some() {
                    found.insert(stringify!($id));
                }
            )*
            Ok(found)
        }
    };
}

const ARCHIVED: &str = "11111111-1111-4111-8111-111111111111";
const LIVE: &str = "22222222-2222-4222-8222-222222222222";
const KEY: &str = "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa";
const REVISION: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const ITEM: &str = "cccccccc-cccc-4ccc-8ccc-cccccccccccc";
const JOB: &str = "eeeeeeee-eeee-4eee-8eee-eeeeeeeeeeee";
const UPLOAD: &str = "dddddddd-dddd-4ddd-8ddd-dddddddddddd";
const CONNECTOR: &str = "ffffffff-ffff-4fff-8fff-ffffffffffff";

/// The mutations an archived workspace allows that name it in their targets or body, each sent
/// to the archived workspace.
const ALLOWED_MUTATIONS: [&str; 9] = [
    "archive_workspace",
    "unarchive_workspace",
    "purge_workspace",
    "export_workspace",
    "backup_workspace",
    "purge_item",
    "cancel_job",
    "rebuild_index",
    "export_view",
];

/// Mutations of a workspace an archived workspace refuses: a commit, a draft, an upload, an
/// import, a job retried, and its own metadata.
const REFUSED_MUTATIONS: [&str; 6] = [
    "create_item",
    "save_draft",
    "create_upload",
    "start_import",
    "retry_job",
    "update_workspace",
];

fn principal(subject: &str, route: AccessRoute) -> Result<Principal, IdentityError> {
    Ok(Principal {
        subject: subject.to_owned(),
        tenant_id: tenant("tenant-local")?,
        route,
        client_id: None,
        delegation: None,
    })
}

fn archived_at() -> Result<Timestamp, IdentityError> {
    Timestamp::try_from("2026-10-10T08:00:00.000Z".to_owned())
}

/// Alice holds every permission on both workspaces and on the tenant; the first is archived.
fn world() -> Result<(Archiving, FixturePorts), Box<dyn std::error::Error>> {
    let mut table = GrantTable::default();
    let alice = table.workspaces.entry("alice".to_owned()).or_default();
    alice.insert(workspace(ARCHIVED)?, all_permissions());
    alice.insert(workspace(LIVE)?, all_permissions());
    table.tenants.insert("alice".to_owned(), all_permissions());
    let app = Archiving::default();
    app.archive(workspace(ARCHIVED)?, archived_at()?)?;
    Ok((app, FixturePorts::new(table)))
}

async fn call(
    app: &Archiving,
    ports: &FixturePorts,
    principal: &Principal,
    operation: &str,
    input: Value,
) -> Result<Value, ApiError> {
    let caller = Caller {
        principal,
        session_id: None,
    };
    let dispatch_ports = DispatchPorts {
        access: ports.access.as_ref(),
        mutations: ports.mutations.as_ref(),
        events: ports.events.as_ref(),
    };
    dispatch(app, &dispatch_ports, &caller, operation, input).await
}

/// A valid request body of `operation` in `workspace_id`, for the operations these tests send.
fn body(operation: &str, workspace_id: &str) -> Result<Value, String> {
    let body = match operation {
        "create_item" => json!({
            "workspace_id": workspace_id, "base_revision": REVISION, "path": "notes/a.md",
            "title": "a", "type_name": "note", "kind": "note", "body": "text",
            "properties": {}, "idempotency_key": KEY
        }),
        "save_draft" => json!({
            "workspace_id": workspace_id, "item_id": ITEM, "base_revision": REVISION,
            "body": "text", "properties": {}, "idempotency_key": KEY
        }),
        "create_upload" => json!({
            "workspace_id": workspace_id, "filename": "a.pdf", "relative_path": "",
            "size": "10", "idempotency_key": KEY
        }),
        "start_import" => json!({
            "workspace_id": workspace_id, "base_revision": REVISION, "upload_ids": [UPLOAD],
            "destination": "", "idempotency_key": KEY, "apply_naming_rules": false
        }),
        "retry_job" | "cancel_job" => json!({
            "workspace_id": workspace_id, "job_id": JOB, "idempotency_key": KEY
        }),
        "update_workspace" => json!({
            "workspace_id": workspace_id, "base_revision": REVISION, "name": "n",
            "description": "d", "idempotency_key": KEY
        }),
        "archive_workspace" | "purge_workspace" => json!({
            "workspace_id": workspace_id, "base_revision": REVISION, "idempotency_key": KEY
        }),
        "unarchive_workspace" | "backup_workspace" | "rebuild_index" => json!({
            "workspace_id": workspace_id, "idempotency_key": KEY
        }),
        "export_workspace" => json!({
            "workspace_id": workspace_id, "at": { "kind": "latest" }, "include_history": false,
            "idempotency_key": KEY
        }),
        "export_view" => json!({
            "workspace_id": workspace_id, "item_id": ITEM, "at": { "kind": "latest" },
            "idempotency_key": KEY
        }),
        "purge_item" => json!({
            "workspace_id": workspace_id, "item_id": ITEM, "base_revision": REVISION,
            "idempotency_key": KEY
        }),
        "list_items" => json!({
            "workspace_id": workspace_id, "folder": "", "at": { "kind": "latest" },
            "page": { "limit": 10 }
        }),
        "list_events" => json!({ "workspace_id": workspace_id, "page": { "limit": 10 } }),
        "get_job" => json!({ "workspace_id": workspace_id, "job_id": JOB }),
        "create_workspace" => json!({ "name": "n", "description": "d", "idempotency_key": KEY }),
        "backup_installation" => json!({ "idempotency_key": KEY }),
        "create_connector" => json!({
            "label": "agent", "workspace_ids": [workspace_id], "allow_propose": false,
            "idempotency_key": KEY
        }),
        "list_connectors" => json!({ "include_revoked": false }),
        "revoke_connector" => json!({ "connector_id": CONNECTOR, "idempotency_key": KEY }),
        other => return Err(format!("no test body for {other}")),
    };
    Ok(body)
}

#[test]
fn every_mutation_an_archived_workspace_allows_is_sent_by_these_tests() -> TestResult {
    let mutations = mutations()?;
    let allowed: BTreeSet<&str> = ALLOWED_WHILE_ARCHIVED
        .iter()
        .map(|operation| operation.as_str())
        .filter(|operation| mutations.contains(operation))
        .collect();
    let sent: BTreeSet<&str> = ALLOWED_MUTATIONS.into_iter().collect();
    assert_eq!(allowed, sent);
    for refused in REFUSED_MUTATIONS {
        assert!(mutations.contains(refused), "{refused} is a mutation");
        assert!(
            !ALLOWED_WHILE_ARCHIVED
                .iter()
                .any(|operation| operation.as_str() == refused),
            "{refused} is not allowed while archived"
        );
    }
    Ok(())
}

#[tokio::test]
async fn a_mutation_of_an_archived_workspace_is_refused_before_its_handler_and_its_ledger()
-> TestResult {
    for operation in REFUSED_MUTATIONS {
        let (app, ports) = world()?;
        let alice = principal("alice", AccessRoute::LocalOwner)?;
        let refused =
            err_of(call(&app, &ports, &alice, operation, body(operation, ARCHIVED)?).await)?;
        assert_eq!(refused.code, ErrorCode::Conflict, "{operation}");
        let detail = some(refused.detail.as_deref(), "the archived detail")?;
        assert!(
            matches!(detail, ErrorDetail::WorkspaceArchived { archived_at: at } if *at == archived_at()?),
            "{operation}: {detail:?}"
        );
        assert_eq!(app.handlers.call_count(operation)?, 0, "{operation}");
        assert!(
            ports.events.appended()?.is_empty(),
            "{operation}: recorded nowhere"
        );
        assert_eq!(
            app.asked()?.len(),
            1,
            "{operation}: the catalog entry is read once"
        );
        // The ledger never began it: once the workspace is unarchived, the same request runs as
        // a first attempt, not as the resumption of a released one.
        app.unarchive(workspace(ARCHIVED)?)?;
        let _ran = call(&app, &ports, &alice, operation, body(operation, ARCHIVED)?).await;
        let contexts = app.handlers.contexts(operation)?;
        let context = some(contexts.first(), "the handler's run")?;
        assert_eq!(context.attempt, Attempt::First, "{operation}");
    }
    Ok(())
}

#[tokio::test]
async fn every_mutation_an_archived_workspace_allows_reaches_its_handler() -> TestResult {
    for operation in ALLOWED_MUTATIONS {
        let (app, ports) = world()?;
        let alice = principal("alice", AccessRoute::LocalOwner)?;
        // The scripted handler answers no response, so the call fails after the handler ran.
        let _ran = call(&app, &ports, &alice, operation, body(operation, ARCHIVED)?).await;
        assert_eq!(app.handlers.call_count(operation)?, 1, "{operation} ran");
        assert!(
            app.asked()?.is_empty(),
            "{operation}: no archive question asked"
        );
    }
    Ok(())
}

#[tokio::test]
async fn a_read_of_an_archived_workspace_runs_and_asks_no_archive_question() -> TestResult {
    for operation in ["list_items", "list_events", "get_job"] {
        let (app, ports) = world()?;
        let alice = principal("alice", AccessRoute::LocalOwner)?;
        let _ran = call(&app, &ports, &alice, operation, body(operation, ARCHIVED)?).await;
        assert_eq!(app.handlers.call_count(operation)?, 1, "{operation} ran");
        assert!(app.asked()?.is_empty(), "{operation}");
    }
    Ok(())
}

#[tokio::test]
async fn a_mutation_of_a_live_workspace_asks_once_and_runs() -> TestResult {
    let (app, ports) = world()?;
    let alice = principal("alice", AccessRoute::LocalOwner)?;
    let _ran = call(
        &app,
        &ports,
        &alice,
        "create_item",
        body("create_item", LIVE)?,
    )
    .await;
    assert_eq!(app.handlers.call_count("create_item")?, 1);
    let asked = app.asked()?;
    let scope = some(asked.first(), "the question")?;
    assert_eq!(asked.len(), 1);
    assert_eq!(scope.workspace_id, workspace(LIVE)?);
    assert_eq!(scope.tenant_id, tenant("tenant-local")?);
    Ok(())
}

#[tokio::test]
async fn a_caller_refused_by_its_grant_learns_nothing_of_the_archive() -> TestResult {
    let (app, ports) = world()?;
    let bob = principal("bob", AccessRoute::LocalOwner)?;
    let refused = err_of(
        call(
            &app,
            &ports,
            &bob,
            "create_item",
            body("create_item", ARCHIVED)?,
        )
        .await,
    )?;
    assert_eq!(refused.code, ErrorCode::Forbidden);
    assert!(app.asked()?.is_empty(), "authorization comes first");
    Ok(())
}

#[tokio::test]
async fn a_mutation_authorized_at_the_tenant_asks_no_archive_question() -> TestResult {
    for operation in [
        "create_workspace",
        "backup_installation",
        "create_connector",
    ] {
        let (app, ports) = world()?;
        let alice = principal("alice", AccessRoute::LocalOwner)?;
        let _ran = call(&app, &ports, &alice, operation, body(operation, ARCHIVED)?).await;
        assert_eq!(app.handlers.call_count(operation)?, 1, "{operation} ran");
        assert!(app.asked()?.is_empty(), "{operation}");
    }
    Ok(())
}

#[tokio::test]
async fn an_archive_question_that_fails_fails_the_request_before_its_handler() -> TestResult {
    let (app, ports) = world()?;
    app.fail_next(ApiError::new(ErrorCode::NotFound, "no such workspace"))?;
    let alice = principal("alice", AccessRoute::LocalOwner)?;
    let refused = err_of(
        call(
            &app,
            &ports,
            &alice,
            "create_item",
            body("create_item", LIVE)?,
        )
        .await,
    )?;
    assert_eq!(refused.code, ErrorCode::NotFound);
    assert_eq!(app.handlers.call_count("create_item")?, 0);
    Ok(())
}

#[test]
fn only_the_local_owner_passes_the_connector_route_rule() -> TestResult {
    for operation in OperationName::ALL.iter().copied() {
        for route in [
            AccessRoute::LocalOwner,
            AccessRoute::BrowserSession,
            AccessRoute::McpDelegation,
            AccessRoute::Service,
        ] {
            let caller = principal("alice", route.clone())?;
            let outcome = check_local_owner_route(&caller, operation);
            let refused = LOCAL_OWNER_ONLY.contains(&operation) && route != AccessRoute::LocalOwner;
            assert_eq!(
                outcome.as_ref().err().map(|error| error.code.clone()),
                refused.then_some(ErrorCode::Forbidden),
                "{} on {route:?}",
                operation.as_str()
            );
        }
    }
    Ok(())
}

#[tokio::test]
async fn a_connector_operation_on_any_route_but_the_local_owners_is_refused_and_recorded()
-> TestResult {
    for operation in ["create_connector", "list_connectors", "revoke_connector"] {
        for route in [
            AccessRoute::BrowserSession,
            AccessRoute::McpDelegation,
            AccessRoute::Service,
        ] {
            let (app, ports) = world()?;
            let caller = principal("alice", route.clone())?;
            let refused =
                err_of(call(&app, &ports, &caller, operation, body(operation, LIVE)?).await)?;
            assert_eq!(
                refused.code,
                ErrorCode::Forbidden,
                "{operation} on {route:?}"
            );
            assert_eq!(app.handlers.call_count(operation)?, 0);
            let appended = ports.events.appended()?;
            let (log, event) = some(appended.first(), "the refusal event")?;
            assert_eq!(*log, EventScope::Tenant(tenant("tenant-local")?));
            assert_eq!(
                event.operation.map(OperationName::as_str),
                Some(operation)
            );
        }
        let (app, ports) = world()?;
        let owner = principal("alice", AccessRoute::LocalOwner)?;
        let _ran = call(&app, &ports, &owner, operation, body(operation, LIVE)?).await;
        assert_eq!(
            app.handlers.call_count(operation)?,
            1,
            "the local owner runs {operation}"
        );
    }
    Ok(())
}

#[tokio::test]
async fn a_refused_create_connector_names_the_listed_workspace_that_refused_it() -> TestResult {
    let unreadable = "33333333-3333-4333-8333-333333333333";
    let mut table = GrantTable::default();
    let alice = table.workspaces.entry("alice".to_owned()).or_default();
    alice.insert(workspace(LIVE)?, vec![Permission::Read]);
    table.tenants.insert("alice".to_owned(), all_permissions());
    let ports = FixturePorts::new(table);
    let app = Archiving::default();
    let owner = principal("alice", AccessRoute::LocalOwner)?;
    for (listed, propose, field) in [
        (
            vec![LIVE, unreadable, unreadable],
            false,
            "/workspace_ids/1",
        ),
        (vec![unreadable, LIVE], false, "/workspace_ids/0"),
        (vec![LIVE], true, "/workspace_ids/0"),
    ] {
        let request = json!({
            "label": "agent", "workspace_ids": listed, "allow_propose": propose,
            "idempotency_key": KEY
        });
        let refused = err_of(call(&app, &ports, &owner, "create_connector", request).await)?;
        assert_eq!(refused.code, ErrorCode::Forbidden);
        assert_eq!(
            refused.field.as_deref(),
            Some(field),
            "{listed:?}, propose {propose}"
        );
    }
    // A refusal at the tenant names no entry.
    let bob = principal("bob", AccessRoute::LocalOwner)?;
    let refused = err_of(
        call(
            &app,
            &ports,
            &bob,
            "create_connector",
            body("create_connector", LIVE)?,
        )
        .await,
    )?;
    assert_eq!(refused.code, ErrorCode::Forbidden);
    assert_eq!(refused.field, None);
    assert_eq!(app.handlers.call_count("create_connector")?, 0);
    Ok(())
}

okf_jawn_contract::for_each_operation!(mutation_ids);

#[path = "support/archiving.rs"]
mod archiving;
#[path = "../../../tests/support/check.rs"]
mod check;
mod support;
