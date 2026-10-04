//! Test-only response fixture for checking bindings; never linked into production.

use std::collections::BTreeMap;
use okf_jawn_contract::access::Principal;
use okf_jawn_contract::error::{ApiError, ErrorCode};
use okf_jawn_core::ports::{Application, PortFuture};
use serde_json::Value;

pub struct FixtureApplication { pub responses: BTreeMap<String, Value> }
macro_rules! fixture_operations {
    ($(($id:ident, $request:ty, $response:ty, $path:literal, $label:literal, $alias:literal, $visibility:literal,
        $permission:ident, $ui:literal, $status:literal, $description:literal)),* $(,)?) => {
        impl Application for FixtureApplication {
            $(fn $id<'a>(&'a self, _principal: &'a Principal, _request: $request) -> PortFuture<'a, $response> {
                Box::pin(async move {
                    let value = self.responses.get(stringify!($id)).cloned()
                        .ok_or_else(|| ApiError::new(ErrorCode::Unavailable,"No test response configured"))?;
                    serde_json::from_value(value).map_err(|error| ApiError::new(ErrorCode::Internal,error.to_string()))
                })
            })*
        }
    };
}
okf_jawn_contract::for_each_operation!(fixture_operations);
