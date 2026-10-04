//! Structured transport-independent application errors.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

/// Stable failure classifications; HTTP and MCP adapters preserve these.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, ToSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    /// Input violates the declared contract.
    InvalidInput,
    /// No acceptable identity was established.
    Unauthenticated,
    /// The principal lacks the required capability.
    Forbidden,
    /// The resource is absent or not visible to this principal.
    NotFound,
    /// A precondition, revision, or idempotency conflict occurred.
    Conflict,
    /// A configured resource limit was exceeded.
    TooLarge,
    /// The requested representation or format is unsupported.
    Unsupported,
    /// A configured integration is unavailable.
    Unavailable,
    /// The operation was cancelled.
    Cancelled,
    /// An unexpected server failure occurred without exposing secrets.
    Internal,
}

/// A safe error response, not an internal backtrace.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct ApiError {
    /// Stable code for programmatic handling.
    pub code: ErrorCode,
    /// Safe explanation for the caller.
    pub message: String,
    /// Input field needing correction.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub field: Option<String>,
    /// Correlation identifier, not a secret.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_id: Option<String>,
}

impl std::fmt::Display for ApiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}
impl std::error::Error for ApiError {}
impl ApiError {
    /// Construct a safe error without source-code details or credentials.
    #[must_use]
    pub fn new(code: ErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            field: None,
            request_id: None,
        }
    }
}
