//! Counting Application fixture for dispatch authorization and ledger tests.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;

use okf_jawn_contract::error::{ApiError, ErrorCode};
use okf_jawn_core::context::OperationContext;
use okf_jawn_core::ports::{Application, PortFuture};
use serde_json::Value;

macro_rules! counting_operations {
    ($(($id:ident, $request:ty, $response:ty, $path:literal, $label:literal, $alias:literal, $visibility:literal,
        $permission:ident, $ui:literal, $status:literal, $description:literal)),* $(,)?) => {
        impl Application for CountingApplication {
            $(fn $id<'a>(&'a self, context: &'a OperationContext, _request: $request) -> PortFuture<'a, $response> {
                Box::pin(async move {
                    self.record(stringify!($id), context);
                    let value = self.responses.lock().map_err(|_| {
                        ApiError::new(ErrorCode::Internal, "responses lock poisoned")
                    })?.get(stringify!($id)).cloned().ok_or_else(|| {
                        ApiError::new(ErrorCode::Unavailable, "No test response configured")
                    })?;
                    serde_json::from_value(value).map_err(|error| {
                        ApiError::new(ErrorCode::Internal, error.to_string())
                    })
                })
            })*
        }
    };
}

/// Application that counts handler invocations and returns configured JSON responses.
#[derive(Default)]
pub struct CountingApplication {
    /// Responses keyed by operation id.
    pub responses: Mutex<BTreeMap<String, Value>>,
    /// Call counts keyed by operation id.
    pub calls: Mutex<BTreeMap<String, usize>>,
    /// Total handler invocations.
    pub total: AtomicUsize,
    /// Last mutation id observed by a handler, when present.
    pub last_mutation: Mutex<Option<okf_jawn_contract::identity::MutationId>>,
}

impl CountingApplication {
    /// Construct with no configured responses.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Configure a successful JSON response for one operation.
    ///
    /// # Errors
    /// Returns when the lock is poisoned.
    pub fn set_response(&self, operation: &str, value: Value) -> Result<(), ApiError> {
        self.responses
            .lock()
            .map_err(|_| ApiError::new(ErrorCode::Internal, "responses lock poisoned"))?
            .insert(operation.to_owned(), value);
        Ok(())
    }

    /// Read the call count for one operation.
    ///
    /// # Errors
    /// Returns when the lock is poisoned.
    pub fn call_count(&self, operation: &str) -> Result<usize, ApiError> {
        Ok(self
            .calls
            .lock()
            .map_err(|_| ApiError::new(ErrorCode::Internal, "calls lock poisoned"))?
            .get(operation)
            .copied()
            .unwrap_or(0))
    }

    fn record(&self, operation: &str, context: &OperationContext) {
        self.total.fetch_add(1, Ordering::SeqCst);
        if let Ok(mut calls) = self.calls.lock() {
            *calls.entry(operation.to_owned()).or_insert(0) += 1;
        }
        if let Ok(mut last) = self.last_mutation.lock() {
            *last = context.mutation;
        }
    }
}

okf_jawn_contract::for_each_operation!(counting_operations);
