//! Runtime-safe operation metadata projected from the canonical declaration table.

use serde::Serialize;

use crate::access::Permission;

macro_rules! collect_operations {
    ($(($id:ident, $request:ty, $response:ty, $path:literal, $label:literal, $alias:literal, $visibility:literal,
        $permission:ident, $ui:literal, $status:literal, $description:literal)),* $(,)?) => {
        /// Return canonical operations without opening storage or running an HTTP server.
        #[must_use]
        pub fn operations() -> Vec<OperationInfo> {
            vec![$(OperationInfo {
                id: stringify!($id), path: $path, label: $label, alias: $alias, visibility: $visibility,
                permission: Permission::$permission, ui: $ui, success_status: $status,
                description: $description,
            }),*]
        }
    };
}

/// An operation's shared names and declared execution policy.
#[derive(Debug, Clone, Serialize)]
pub struct OperationInfo {
    /// Canonical Rust, OpenAPI, and telemetry operation identifier.
    pub id: &'static str,
    /// Declared HTTP endpoint.
    pub path: &'static str,
    /// Human-facing operator vocabulary.
    pub label: &'static str,
    /// Optional concise tool and CLI alias; empty means not model-exposed.
    pub alias: &'static str,
    /// Model-facing, app-only, or not exposed through MCP.
    pub visibility: &'static str,
    /// Minimum application capability, independent of HTTP method.
    pub permission: Permission,
    /// Optional approved MCP App presentation key.
    pub ui: &'static str,
    /// HTTP success status.
    pub success_status: u16,
    /// Shared operation documentation.
    pub description: &'static str,
}

crate::for_each_operation!(collect_operations);
