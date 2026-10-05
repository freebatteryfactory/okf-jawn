//! Scripted `Application` for dispatch and binding tests.
//!
//! It records every handler call with the context dispatch built, and answers from a configured
//! response, a one-shot error, or a handler parked until the test resumes it.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Mutex;

use okf_jawn_contract::error::{ApiError, ErrorCode};
use okf_jawn_core::context::OperationContext;
use okf_jawn_core::ports::{Application, PortFuture};
use serde_json::Value;
use tokio::sync::Notify;

use super::lock;

macro_rules! counting_operations {
    ($(($id:ident, $request:ty, $response:ty, $path:literal, $label:literal, $alias:literal,
        $operator:literal, $visibility:literal, $permission:ident, $ui:literal, $status:literal,
        $destructive:literal, $description:literal)),* $(,)?) => {
        impl Application for CountingApplication {
            $(fn $id<'a>(&'a self, context: &'a OperationContext, _request: $request) -> PortFuture<'a, $response> {
                Box::pin(async move {
                    let value = self.run(stringify!($id), context).await?;
                    serde_json::from_value(value)
                        .map_err(|error| ApiError::new(ErrorCode::Internal, error.to_string()))
                })
            })*
        }
    };
}

/// `Application` that records handler calls and plays a per-operation script.
#[derive(Default)]
pub struct CountingApplication {
    responses: Mutex<BTreeMap<String, Value>>,
    failures: Mutex<BTreeMap<String, ApiError>>,
    parked: Mutex<BTreeSet<String>>,
    calls: Mutex<Vec<(String, OperationContext)>>,
    entered: Notify,
    resumed: Notify,
}

impl CountingApplication {
    /// Construct with no configured responses.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Configure the successful JSON response of one operation.
    ///
    /// # Errors
    /// Returns when the lock is poisoned.
    pub fn set_response(&self, operation: &str, value: Value) -> Result<(), ApiError> {
        lock(&self.responses, "responses")?.insert(operation.to_owned(), value);
        Ok(())
    }

    /// Make the next call of `operation` fail with `error`; later calls answer normally.
    ///
    /// # Errors
    /// Returns when the lock is poisoned.
    pub fn fail_once(&self, operation: &str, error: ApiError) -> Result<(), ApiError> {
        lock(&self.failures, "failures")?.insert(operation.to_owned(), error);
        Ok(())
    }

    /// Make the next call of `operation` wait inside its handler until [`Self::resume`].
    ///
    /// # Errors
    /// Returns when the lock is poisoned.
    pub fn park_next(&self, operation: &str) -> Result<(), ApiError> {
        lock(&self.parked, "parked operations")?.insert(operation.to_owned());
        Ok(())
    }

    /// Resolve once a parked handler has been entered.
    pub async fn entered(&self) {
        self.entered.notified().await;
    }

    /// Let the parked handler continue.
    pub fn resume(&self) {
        self.resumed.notify_one();
    }

    /// The context of every call of `operation`, oldest first.
    ///
    /// # Errors
    /// Returns when the lock is poisoned.
    pub fn contexts(&self, operation: &str) -> Result<Vec<OperationContext>, ApiError> {
        Ok(lock(&self.calls, "calls")?
            .iter()
            .filter(|(name, _)| name == operation)
            .map(|(_, context)| context.clone())
            .collect())
    }

    /// Number of times the handler of `operation` ran.
    ///
    /// # Errors
    /// Returns when the lock is poisoned.
    pub fn call_count(&self, operation: &str) -> Result<usize, ApiError> {
        Ok(self.contexts(operation)?.len())
    }

    async fn run(&self, operation: &str, context: &OperationContext) -> Result<Value, ApiError> {
        lock(&self.calls, "calls")?.push((operation.to_owned(), context.clone()));
        let parked = lock(&self.parked, "parked operations")?.remove(operation);
        if parked {
            self.entered.notify_one();
            self.resumed.notified().await;
        }
        let failure = lock(&self.failures, "failures")?.remove(operation);
        if let Some(error) = failure {
            return Err(error);
        }
        lock(&self.responses, "responses")?
            .get(operation)
            .cloned()
            .ok_or_else(|| ApiError::new(ErrorCode::Unavailable, "No test response configured"))
    }
}

okf_jawn_contract::for_each_operation!(counting_operations);

#[cfg(test)]
mod tests {
    use okf_jawn_contract::access::{AccessRoute, Principal};
    use okf_jawn_contract::error::{ApiError, ErrorCode};
    use okf_jawn_contract::health::HealthRequest;
    use okf_jawn_contract::identity::{IdentityError, TenantId};
    use okf_jawn_contract::metadata::OperationName;
    use okf_jawn_core::context::{Attempt, OperationContext};
    use okf_jawn_core::ports::Application;
    use serde_json::json;

    use super::CountingApplication;
    use crate::check::{TestResult, err_of};

    fn context() -> Result<OperationContext, IdentityError> {
        Ok(OperationContext {
            principal: Principal {
                subject: "alice".to_owned(),
                tenant_id: TenantId::try_from("tenant-local".to_owned())?,
                route: AccessRoute::LocalOwner,
                client_id: None,
                delegation: None,
            },
            session_id: None,
            operation: OperationName::GetHealth,
            tenant: None,
            grants: Vec::new(),
            mutation: None,
            attempt: Attempt::First,
        })
    }

    #[tokio::test]
    async fn scripted_handler_records_calls_fails_once_then_answers() -> TestResult {
        let app = CountingApplication::new();
        let context = context()?;
        let unconfigured = err_of(app.get_health(&context, HealthRequest {}).await)?;
        assert_eq!(unconfigured.code, ErrorCode::Unavailable);

        app.set_response(
            "get_health",
            json!({ "status": "alive", "version": "test" }),
        )?;
        app.fail_once("get_health", ApiError::new(ErrorCode::Conflict, "scripted"))?;
        let scripted = err_of(app.get_health(&context, HealthRequest {}).await)?;
        assert_eq!(scripted.code, ErrorCode::Conflict);
        let answered = app.get_health(&context, HealthRequest {}).await?;
        assert_eq!(answered.status, "alive");

        assert_eq!(app.call_count("get_health")?, 3);
        assert_eq!(app.call_count("list_items")?, 0);
        let recorded = app.contexts("get_health")?;
        assert_eq!(
            recorded.first().map(|seen| seen.attempt),
            Some(Attempt::First)
        );
        Ok(())
    }

    #[tokio::test]
    async fn parked_handler_waits_until_it_is_resumed() -> TestResult {
        let app = CountingApplication::new();
        let context = context()?;
        app.set_response(
            "get_health",
            json!({ "status": "alive", "version": "test" }),
        )?;
        app.park_next("get_health")?;
        let mut call = app.get_health(&context, HealthRequest {});
        tokio::select! {
            biased;
            outcome = &mut call => {
                return Err(format!("the handler finished while parked: {outcome:?}").into());
            }
            () = app.entered() => {}
        }
        app.resume();
        assert_eq!(call.await?.status, "alive");
        Ok(())
    }
}
