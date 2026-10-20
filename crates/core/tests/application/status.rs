//! Health answers without asking a dependency; readiness reports the probe honestly with the
//! configured sandbox origin; the session names the principal and when CSRF applies.

use okf_jawn_contract::access::AccessRoute;
use okf_jawn_contract::common::Empty;
use okf_jawn_contract::error::{ApiError, ErrorCode};
use okf_jawn_contract::health::{
    DependencyStatus, HealthRequest, ReadinessRequest, ReadinessResponse,
};
use okf_jawn_contract::import::DEFAULT_MAX_UPLOAD_BYTES;
use okf_jawn_contract::metadata::OperationName;
use okf_jawn_core::application::ApplicationConfig;
use okf_jawn_core::ports::Application;

use crate::check::{TestResult, err_of, some};
use crate::fixture::{ALICE, SANDBOX_ORIGIN, World, alice, config, context};

fn dependency(name: &str, ready: bool) -> DependencyStatus {
    DependencyStatus {
        name: name.to_owned(),
        ready,
        message: if ready { "ready" } else { "down" }.to_owned(),
    }
}

#[tokio::test]
async fn get_health_answers_alive_without_asking_any_dependency() -> TestResult {
    let world = World::new()?;
    world.readiness.fail(ApiError::new(
        ErrorCode::Unavailable,
        "the records store is down",
    ))?;
    let health = world
        .service
        .get_health(&alice(OperationName::GetHealth, None)?, HealthRequest {})
        .await?;
    assert_eq!(health.status, "alive");
    assert_eq!(health.version, env!("CARGO_PKG_VERSION"));
    assert_eq!(world.readiness.probes(), 0);
    Ok(())
}

#[tokio::test]
async fn get_readiness_reports_the_probe_and_the_configured_sandbox_origin() -> TestResult {
    let world = World::new()?;
    world.readiness.answer(ReadinessResponse {
        ready: true,
        dependencies: vec![dependency("records", true), dependency("index", true)],
        sandbox_origin: None,
    })?;
    let readiness = world
        .service
        .get_readiness(
            &alice(OperationName::GetReadiness, None)?,
            ReadinessRequest {},
        )
        .await?;
    assert!(readiness.ready);
    let names: Vec<&str> = readiness
        .dependencies
        .iter()
        .map(|dependency| dependency.name.as_str())
        .collect();
    assert_eq!(names, vec!["records", "index"]);
    let origin = some(readiness.sandbox_origin.as_ref(), "the sandbox origin")?;
    assert_eq!(origin.origin, SANDBOX_ORIGIN);
    assert_eq!(world.readiness.probes(), 1);
    Ok(())
}

#[tokio::test]
async fn get_readiness_is_never_ready_while_the_probe_or_a_dependency_is_not() -> TestResult {
    let world = World::new()?;
    let caller = alice(OperationName::GetReadiness, None)?;
    for (ready, dependencies) in [
        (false, vec![dependency("records", true)]),
        (
            true,
            vec![dependency("records", true), dependency("objects", false)],
        ),
    ] {
        world.readiness.answer(ReadinessResponse {
            ready,
            dependencies: dependencies.clone(),
            sandbox_origin: None,
        })?;
        let readiness = world
            .service
            .get_readiness(&caller, ReadinessRequest {})
            .await?;
        assert!(!readiness.ready, "{dependencies:?}");
        assert_eq!(readiness.dependencies.len(), dependencies.len());
    }
    world.readiness.fail(ApiError::new(
        ErrorCode::Unavailable,
        "the probe did not answer",
    ))?;
    let failed = err_of(
        world
            .service
            .get_readiness(&caller, ReadinessRequest {})
            .await,
    )?;
    assert_eq!(failed.code, ErrorCode::Unavailable);
    Ok(())
}

#[tokio::test]
async fn get_session_returns_the_principal_and_needs_csrf_only_in_a_browser_session() -> TestResult
{
    let world = World::new()?;
    let mut browser = alice(OperationName::GetSession, None)?;
    browser.session_id = Some("session-1".to_owned());
    let session = world.service.get_session(&browser, Empty {}).await?;
    assert_eq!(session.principal.subject, ALICE);
    assert_eq!(session.principal.route, AccessRoute::LocalOwner);
    assert!(session.csrf_required);
    let agent = context(
        OperationName::GetSession,
        "agent",
        AccessRoute::McpDelegation,
        None,
    )?;
    let session = world.service.get_session(&agent, Empty {}).await?;
    assert_eq!(session.principal.subject, "agent");
    assert_eq!(session.principal.route, AccessRoute::McpDelegation);
    assert!(!session.csrf_required);
    Ok(())
}

/// Ruling 4: the session reports the limits the deployment applies: its own upload limit, the
/// contract's default unless it set another, and the contract's body and text bounds.
#[tokio::test]
async fn get_session_reports_the_deployments_limits() -> TestResult {
    let caller = alice(OperationName::GetSession, None)?;
    let session = World::new()?.service.get_session(&caller, Empty {}).await?;
    assert_eq!(
        session.limits.upload_bytes,
        DEFAULT_MAX_UPLOAD_BYTES.to_string()
    );
    assert_eq!(session.limits.request_body_bytes, 16_777_216);
    assert_eq!(session.limits.item_text_bytes, 8_388_608);
    let small = World::with_config(ApplicationConfig {
        max_upload_bytes: 1024,
        ..config()
    })?;
    let session = small.service.get_session(&caller, Empty {}).await?;
    assert_eq!(session.limits.upload_bytes, "1024");
    let Err(refused) = World::with_config(ApplicationConfig {
        max_upload_bytes: 0,
        ..config()
    }) else {
        return Err("a deployment that allows no upload bytes must be refused".into());
    };
    assert!(refused.to_string().contains("some bytes"), "{refused}");
    Ok(())
}
