//! Identity is read from each HTTP request, and only an MCP delegation reaches this server.
//!
//! Every request here goes through the Streamable HTTP service with the `Principal` placed in
//! the request's extensions, where the server's identity middleware puts it.

use okf_jawn_contract::access::{AccessRoute, Permission};
use okf_jawn_contract::error::ErrorCode;
use okf_jawn_contract::events::EventKind;
use okf_jawn_contract::metadata::OperationName;
use okf_jawn_core::events::EventScope;
use okf_jawn_mcp::APP_URI;
use serde_json::json;
use tokio::task::JoinSet;

use crate::check::{TestResult, some};
use crate::support::fakes::{Call, FakeAccess, World};
use crate::support::fixtures::{
    HOST, agent, bundle, call, call_result, http_service, initialize, object_response, post,
    principal, read_object_input, refusal_of, result_of, rpc, rpc_refusal,
};

const EVERY_PERMISSION: &[Permission] = &[
    Permission::Read,
    Permission::Write,
    Permission::Propose,
    Permission::Approve,
    Permission::Review,
    Permission::Admin,
];

#[tokio::test]
async fn a_call_without_a_principal_is_unauthenticated() -> TestResult {
    let world = World::new(FakeAccess::default().granting("alice", EVERY_PERMISSION));
    world
        .application
        .answer_with("get_object", object_response())?;
    let service = http_service(&world, bundle("identity-none.html", "<p>app</p>")?)?;
    let reply = post(
        &service,
        None,
        HOST,
        &[],
        &call(1, "read_object", &read_object_input()),
    )
    .await?;
    assert_eq!(
        refusal_of(&call_result(&reply)?)?.code,
        ErrorCode::Unauthenticated
    );
    assert_eq!(world.application.calls()?, Vec::new());
    Ok(())
}

#[tokio::test]
async fn every_route_but_mcp_delegation_is_forbidden_and_recorded() -> TestResult {
    let routes = [
        AccessRoute::BrowserSession,
        AccessRoute::LocalOwner,
        AccessRoute::Service,
    ];
    for route in routes {
        // The subject holds every permission, so only the route can refuse it.
        let world = World::new(FakeAccess::default().granting("owner", EVERY_PERMISSION));
        world
            .application
            .answer_with("get_object", object_response())?;
        let service = http_service(&world, bundle("identity-routes.html", "<p>app</p>")?)?;
        let caller = principal("owner", route.clone())?;
        let reply = post(
            &service,
            Some(caller.clone()),
            HOST,
            &[],
            &call(1, "read_object", &read_object_input()),
        )
        .await?;
        assert_eq!(
            refusal_of(&call_result(&reply)?)?.code,
            ErrorCode::Forbidden,
            "{route:?}"
        );
        assert_eq!(world.application.calls()?, Vec::new(), "{route:?}");
        let appended = world.events.appended()?;
        assert_eq!(appended.len(), 1, "{route:?}");
        let (scope, event) = some(appended.first(), "the recorded refusal")?;
        assert_eq!(scope, &EventScope::Tenant(caller.tenant_id.clone()));
        assert_eq!(event.kind, EventKind::PermissionDenied);
        assert_eq!(event.operation, Some(OperationName::GetObject));
        let actor = some(event.actor.as_ref(), "the refused actor")?;
        assert_eq!(actor.route, route);
    }
    Ok(())
}

#[tokio::test]
async fn identity_is_read_from_each_request() -> TestResult {
    let world = World::new(FakeAccess::default().granting("alice", &[Permission::Read]));
    world
        .application
        .answer_with("get_object", object_response())?;
    let service = http_service(&world, bundle("identity-each.html", "<p>app</p>")?)?;
    let request = call(1, "read_object", &read_object_input());
    let first = post(&service, Some(agent("alice")?), HOST, &[], &request).await?;
    assert_eq!(call_result(&first)?.is_error, Some(false));
    let anonymous = post(&service, None, HOST, &[], &request).await?;
    assert_eq!(
        refusal_of(&call_result(&anonymous)?)?.code,
        ErrorCode::Unauthenticated
    );
    let stranger = post(&service, Some(agent("mallory")?), HOST, &[], &request).await?;
    assert_eq!(
        refusal_of(&call_result(&stranger)?)?.code,
        ErrorCode::Forbidden
    );
    assert_eq!(
        world.application.calls()?,
        vec![Call {
            operation: "get_object",
            subject: "alice".to_owned()
        }]
    );
    Ok(())
}

/// Interleaved requests from alice, mallory and nobody through one service each answer as their
/// own caller. A handler that kept the latest caller in shared state passes the sequential test.
#[tokio::test]
async fn concurrent_callers_each_keep_their_own_identity() -> TestResult {
    let world = World::new(FakeAccess::default().granting("alice", &[Permission::Read]));
    world
        .application
        .answer_with("get_object", object_response())?;
    let service = http_service(&world, bundle("identity-concurrent.html", "<p>app</p>")?)?;
    let callers = [Some(agent("alice")?), Some(agent("mallory")?), None];
    let mut requests = JoinSet::new();
    for (id, caller) in (0_u64..30).zip(callers.iter().cycle()) {
        let caller = caller.clone();
        let service = service.clone();
        let request = call(id, "read_object", &read_object_input());
        requests.spawn(async move {
            let subject = caller.as_ref().map(|caller| caller.subject.clone());
            let reply = post(&service, caller, HOST, &[], &request)
                .await
                .map_err(|error| error.to_string())?;
            let result = call_result(&reply).map_err(|error| error.to_string())?;
            Ok::<_, String>((subject, result))
        });
    }
    let mut served = Vec::new();
    while let Some(joined) = requests.join_next().await {
        let (subject, result) = joined??;
        match subject.as_deref() {
            Some("alice") => {
                assert_eq!(result.is_error, Some(false), "{result:?}");
                served.push(result);
            }
            Some(_) => assert_eq!(refusal_of(&result)?.code, ErrorCode::Forbidden),
            None => assert_eq!(refusal_of(&result)?.code, ErrorCode::Unauthenticated),
        }
    }
    let calls = world.application.calls()?;
    assert_eq!(served.len(), 10);
    assert_eq!(calls.len(), served.len());
    assert!(
        calls.iter().all(|call| call.subject == "alice"),
        "{calls:?}"
    );
    Ok(())
}

#[tokio::test]
async fn every_mcp_request_needs_an_agent_identity() -> TestResult {
    let world = World::new(FakeAccess::default().granting("owner", EVERY_PERMISSION));
    let service = http_service(&world, bundle("identity-every-request.html", "<p>app</p>")?)?;
    let browser = principal("owner", AccessRoute::BrowserSession)?;
    let requests = [
        initialize(1),
        rpc(2, "tools/list", &json!({})),
        rpc(3, "resources/list", &json!({})),
        rpc(4, "resources/read", &json!({"uri": APP_URI})),
    ];
    for request in &requests {
        let method = some(request.get("method"), "the request's method")?;
        let anonymous = post(&service, None, HOST, &[], request).await?;
        assert_eq!(
            rpc_refusal(&anonymous)?.code,
            ErrorCode::Unauthenticated,
            "{method}"
        );
        let session = post(&service, Some(browser.clone()), HOST, &[], request).await?;
        assert_eq!(
            rpc_refusal(&session)?.code,
            ErrorCode::Forbidden,
            "{method}"
        );
    }
    // Each browser-session refusal is recorded once, in the tenant log; none is an operation.
    let appended = world.events.appended()?;
    assert_eq!(appended.len(), requests.len());
    for (scope, event) in &appended {
        assert_eq!(scope, &EventScope::Tenant(browser.tenant_id.clone()));
        assert_eq!(event.kind, EventKind::PermissionDenied);
        assert_eq!(event.operation, None);
        let actor = some(event.actor.as_ref(), "the refused actor")?;
        assert_eq!(actor.route, AccessRoute::BrowserSession);
    }
    // The same requests from an agent are served.
    for request in &requests {
        let served = post(&service, Some(agent("owner")?), HOST, &[], request).await?;
        result_of(&served)?;
    }
    assert_eq!(world.application.calls()?, Vec::new());
    Ok(())
}

#[tokio::test]
async fn no_session_is_issued_so_none_can_be_reused_by_another_principal() -> TestResult {
    let world = World::new(FakeAccess::default().granting("alice", &[Permission::Read]));
    world
        .application
        .answer_with("get_object", object_response())?;
    let service = http_service(&world, bundle("identity-session.html", "<p>app</p>")?)?;
    let opened = post(&service, Some(agent("alice")?), HOST, &[], &initialize(1)).await?;
    let info = result_of(&opened)?;
    assert!(info.get("serverInfo").is_some(), "{info}");
    assert_eq!(
        opened.headers.get("mcp-session-id"),
        None,
        "a session was issued"
    );
    // A request that names a session id gains nothing from it: it is served as its own caller.
    let reused = post(
        &service,
        Some(agent("mallory")?),
        HOST,
        &[("mcp-session-id", "alice-session")],
        &call(2, "read_object", &read_object_input()),
    )
    .await?;
    assert_eq!(
        refusal_of(&call_result(&reused)?)?.code,
        ErrorCode::Forbidden
    );
    assert_eq!(world.application.calls()?, Vec::new());
    Ok(())
}
