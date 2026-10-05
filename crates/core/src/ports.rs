//! Application boundaries own typed operations; storage and transport implement them.

use std::future::Future;
use std::pin::Pin;

use okf_jawn_contract::error::ApiError;

use crate::context::OperationContext;

macro_rules! application_port {
    ($(($id:ident, $request:ty, $response:ty, $path:literal, $label:literal, $alias:literal, $visibility:literal,
        $permission:ident, $ui:literal, $status:literal, $description:literal)),* $(,)?) => {
        /// Complete application interface; each method must implement its documented behavior.
        pub trait Application: Send + Sync {
            $(#[doc = $description]
            #[doc = "\n# Errors\nReturns a structured validation, permission, conflict, or execution error."]
            fn $id<'a>(&'a self, context: &'a OperationContext, request: $request)
                -> PortFuture<'a, $response>;)*
        }
    };
}

/// An owned async operation result with no framework-specific task type.
pub type PortFuture<'a, T> = Pin<Box<dyn Future<Output = Result<T, ApiError>> + Send + 'a>>;

okf_jawn_contract::for_each_operation!(application_port);
