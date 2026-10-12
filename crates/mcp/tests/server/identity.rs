//! Identity is read from each HTTP request, and only an MCP delegation reaches a tool.
//!
//! Every request here goes through the Streamable HTTP service with the `Principal` placed in
//! the request's extensions, where the server's identity middleware puts it.

use okf_jawn_contract::access::{AccessRoute, Permission};
use okf_jawn_contract::error::ErrorCode;
use okf_jawn_contract::events::EventKind;
use okf_jawn_contract::metadata::OperationName;
use okf_jawn_core::events::EventScope;

use crate::check::{TestResult, some};
use crate::support::fakes::{Call, FakeAccess, World};
use crate::support::fixtures::{
    HOST, agent, bundle, call, call_result, http_service, initialize, object_response, post,
    principal, read_object_input, refusal_of, result_of,
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
