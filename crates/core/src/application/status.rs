//! Liveness, readiness and the caller's session; none claims product qualification.
//!
//! Health is not readiness (SPEC section 12: "health/readiness distinction"). `get_health`
//! answers that the process responds and asks no dependency, so a store that is down never
//! makes the process look dead. `get_readiness` reports the configured `ReadinessProbe` as it
//! answered: it never reports ready while the probe, or any dependency it lists, is not, and a
//! probe that fails fails the request. The probe leaves the sandbox origin to the server's
//! configuration; the application reports the origin it mints sandbox capabilities on.
//!
//! `get_session` returns the authenticated principal dispatch established, never a credential.
//! A browser write needs the anti-CSRF token exactly when the request arrived in a browser
//! session, whose cookie a cross-site page could otherwise ride (SPEC section 11).

use std::future::Ready;

use okf_jawn_contract::access::SessionResponse;
use okf_jawn_contract::common::Empty;
use okf_jawn_contract::error::ApiError;
use okf_jawn_contract::health::{
    HealthRequest, HealthResponse, ReadinessRequest, ReadinessResponse, SandboxOriginConfig,
};

use super::ApplicationService;
use crate::context::OperationContext;

/// What `get_health` reports for a process that can respond.
const ALIVE: &str = "alive";

/// That the process responds, and its build version; no dependency is asked.
pub(super) fn get_health(
    _service: &ApplicationService,
    _context: &OperationContext,
    _request: HealthRequest,
) -> Ready<Result<HealthResponse, ApiError>> {
    std::future::ready(Ok(HealthResponse {
        status: ALIVE.to_owned(),
        version: env!("CARGO_PKG_VERSION").to_owned(),
    }))
}

/// The configured dependencies' readiness as the probe reports it, with the sandbox origin.
///
/// # Errors
/// Returns the probe's error.
pub(super) async fn get_readiness(
    service: &ApplicationService,
    _context: &OperationContext,
    _request: ReadinessRequest,
) -> Result<ReadinessResponse, ApiError> {
    let probed = service.ports().readiness.probe().await?;
    let ready = probed.ready
        && probed
            .dependencies
            .iter()
            .all(|dependency| dependency.ready);
    Ok(ReadinessResponse {
        ready,
        dependencies: probed.dependencies,
        sandbox_origin: Some(SandboxOriginConfig {
            origin: service.config().sandbox_origin.clone(),
        }),
    })
}

/// The authenticated caller, and whether its writes need the anti-CSRF token.
pub(super) fn get_session(
    _service: &ApplicationService,
    context: &OperationContext,
    _request: Empty,
) -> Ready<Result<SessionResponse, ApiError>> {
    std::future::ready(Ok(SessionResponse {
        principal: context.principal.clone(),
        csrf_required: context.session_id.is_some(),
    }))
}
