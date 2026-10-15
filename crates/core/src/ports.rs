//! Application boundaries own typed operations; storage and transport implement them.
//!
//! Besides one method per operation, `Application` answers one question dispatch asks before a
//! mutation runs: whether the workspace it writes is archived (`Application::archived_at`).

use std::future::Future;
use std::pin::Pin;

use okf_jawn_contract::error::ApiError;
use okf_jawn_contract::identity::Timestamp;

use crate::context::OperationContext;
use crate::storage::StorageScope;

macro_rules! application_port {
    ($(($id:ident, $request:ty, $response:ty, $path:literal, $label:literal, $alias:literal,
        $operator:literal, $visibility:literal, $permission:ident, $ui:literal, $status:literal,
        $destructive:literal, $description:literal)),* $(,)?) => {
        /// Complete application interface; each method must implement its documented behavior.
        pub trait Application: Send + Sync {
            $(#[doc = $description]
            #[doc = "\n# Errors\nReturns a structured validation, permission, conflict, or execution error."]
            fn $id<'a>(&'a self, context: &'a OperationContext, request: $request)
                -> PortFuture<'a, $response>;)*

            /// When the workspace in `scope` was archived; `None` while it is not.
            ///
            /// Dispatch asks this once, after authorization and before the mutation ledger, for
            /// a mutation that writes a workspace and is not in
            /// `operations::ALLOWED_WHILE_ARCHIVED`, and refuses it while the answer is `Some`
            /// (contract ruling 25). `ApplicationService` reads its workspace catalog. The
            /// provided answer, `None`, is for an implementation that keeps no workspaces of its
            /// own, such as a binding's scripted test double: such an implementation applies no
            /// archive rule, so production composes `ApplicationService`, which overrides it.
            ///
            /// # Errors
            /// Returns the catalog's error: `NotFound` for a workspace the tenant does not hold.
            fn archived_at<'a>(&'a self, _scope: &'a StorageScope)
                -> PortFuture<'a, Option<Timestamp>> {
                Box::pin(std::future::ready(Ok(None)))
            }
        }
    };
}

/// An owned async operation result with no framework-specific task type.
pub type PortFuture<'a, T> = Pin<Box<dyn Future<Output = Result<T, ApiError>> + Send + 'a>>;

okf_jawn_contract::for_each_operation!(application_port);
