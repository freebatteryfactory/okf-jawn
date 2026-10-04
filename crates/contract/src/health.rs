//! Liveness and dependency readiness do not claim product qualification.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

/// Read liveness for a running process.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct HealthRequest {
}

/// Basic process metadata only.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct HealthResponse {
    /// alive when the process can respond.
    pub status: String,
    /// Application build version.
    pub version: String,
}

/// Read configured dependency availability.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct ReadinessRequest {
}

/// One configured dependency status.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct DependencyStatus {
    /// Dependency name.
    pub name: String,
    /// Whether required checks succeeded.
    pub ready: bool,
    /// Diagnostic explanation without secrets.
    pub message: String,
}

/// Readiness summary, distinct from acceptance results.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct ReadinessResponse {
    /// All configured mandatory dependencies are ready.
    pub ready: bool,
    /// Individual dependency checks.
    pub dependencies: Vec<DependencyStatus>,
}
