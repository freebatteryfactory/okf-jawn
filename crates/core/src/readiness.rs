//! Dependency readiness probes report configured availability, not product acceptance.

use okf_jawn_contract::health::ReadinessResponse;

use crate::ports::PortFuture;

/// Probe configured dependencies and sandbox origin for readiness responses.
pub trait ReadinessProbe: Send + Sync {
    /// Evaluate configured dependency checks without claiming feature qualification.
    fn probe(&self) -> PortFuture<'_, ReadinessResponse>;
}
