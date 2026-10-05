//! Liveness and dependency readiness do not claim product qualification.
//!
//! Sandboxed HTML and hostile artifacts are served from a configured separate origin
//! (SPEC sections 7 and 11); readiness reports that origin without claiming isolation quality.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Read liveness for a running process.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct HealthRequest {}

/// Basic process metadata only.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct HealthResponse {
    /// alive when the process can respond.
    pub status: String,
    /// Application build version.
    pub version: String,
}

/// Read configured dependency availability.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ReadinessRequest {}

/// One configured dependency status.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DependencyStatus {
    /// Dependency name.
    pub name: String,
    /// Whether required checks succeeded.
    pub ready: bool,
    /// Diagnostic explanation without secrets.
    pub message: String,
}

/// Configured separate origin used to serve sandboxed HTML and hostile artifacts.
///
/// Transport note: the server lane binds a sandbox-origin route that never attaches
/// ambient session cookies or privileged APIs; the workspace-ui iframe loads this
/// origin under sandbox constraints (SPEC sections 7 and 11).
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SandboxOriginConfig {
    /// Absolute origin (scheme + host + optional port), never the application origin.
    pub origin: String,
}

/// Readiness summary, distinct from acceptance results.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ReadinessResponse {
    /// All configured mandatory dependencies are ready.
    pub ready: bool,
    /// Individual dependency checks.
    pub dependencies: Vec<DependencyStatus>,
    /// Configured sandboxed HTML/artifact origin when isolation is enabled.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sandbox_origin: Option<SandboxOriginConfig>,
}
