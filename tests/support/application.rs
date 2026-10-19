//! Test-only response fixture for checking bindings; never linked into production.

use okf_jawn_contract::error::{ApiError, ErrorCode};
use okf_jawn_contract::identity::Timestamp;
use okf_jawn_core::context::OperationContext;
use okf_jawn_core::ports::{Application, PortFuture};
use okf_jawn_core::storage::StorageScope;
use serde_json::Value;
use std::collections::BTreeMap;

macro_rules! fixture_operations {
    ($(($id:ident, $request:ty, $response:ty, $path:literal, $label:literal, $alias:literal,
        $operator:literal, $visibility:literal, $permission:ident, $ui:literal, $status:literal,
        $destructive:literal, $description:literal)),* $(,)?) => {
        impl Application for FixtureApplication {
            $(fn $id<'a>(&'a self, _context: &'a OperationContext, _request: $request) -> PortFuture<'a, $response> {
                Box::pin(async move {
                    let value = self.responses.get(stringify!($id)).cloned()
                        .ok_or_else(|| ApiError::new(ErrorCode::Unavailable,"No test response configured"))?;
                    serde_json::from_value(value).map_err(|error| ApiError::new(ErrorCode::Internal,error.to_string()))
                })
            })*

            /// Every workspace is live: no binding test archives one; the archive rule is core
            /// dispatch's, tested in core.
            fn archived_at<'a>(&'a self, _scope: &'a StorageScope)
                -> PortFuture<'a, Option<Timestamp>> {
                Box::pin(async { Ok(None) })
            }
        }
    };
}

pub struct FixtureApplication {
    pub responses: BTreeMap<String, Value>,
}

okf_jawn_contract::for_each_operation!(fixture_operations);
