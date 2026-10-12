//! Fakes of the ports an MCP server holds, built for this crate's tests; core's are core's.
//!
//! `FakeApplication` answers each operation from a configured response and records who called
//! what. `FakeAccess` grants each known subject a fixed permission set on every workspace and
//! on the tenant, so dispatch's own route and permission rules decide. `FakeLedger` always
//! begins a new mutation. `FakeEvents` records every append.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use okf_jawn_contract::access::{Permission, Principal};
use okf_jawn_contract::error::{ApiError, ErrorCode};
use okf_jawn_contract::events::{Event, ListEventsResponse};
use okf_jawn_contract::identity::{Digest, MutationId, WorkspaceId};
use okf_jawn_core::access::AccessControl;
use okf_jawn_core::context::{OperationContext, TenantGrant, WorkspaceGrant};
use okf_jawn_core::dispatch::new_mutation_id;
use okf_jawn_core::events::{EventLog, EventQuery, EventScope, NewEvent};
use okf_jawn_core::mutations::{BeginOutcome, MutationKey, MutationLease, MutationStore};
use okf_jawn_core::ports::{Application, PortFuture};
use okf_jawn_core::storage::StorageScope;
use okf_jawn_mcp::McpPorts;
use serde_json::{Value, json};

macro_rules! fake_operations {
    ($(($id:ident, $request:ty, $response:ty, $path:literal, $label:literal, $alias:literal,
        $operator:literal, $visibility:literal, $permission:ident, $ui:literal, $status:literal,
        $destructive:literal, $description:literal)),* $(,)?) => {
        impl Application for FakeApplication {
            $(fn $id<'a>(&'a self, context: &'a OperationContext, _request: $request)
                -> PortFuture<'a, $response> {
                Box::pin(async move { self.answer(stringify!($id), context) })
            })*
        }
    };
}

/// One operation the application ran, and for whom.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Call {
    /// The operation id.
    pub operation: &'static str,
    /// The subject of the context dispatch built.
    pub subject: String,
}

/// An application that answers from configured responses and records each call.
#[derive(Default)]
pub struct FakeApplication {
    responses: Mutex<BTreeMap<&'static str, Value>>,
    calls: Mutex<Vec<Call>>,
}

/// Grants by subject; an unknown subject holds nothing.
#[derive(Default)]
pub struct FakeAccess {
    grants: BTreeMap<String, Vec<Permission>>,
}

/// A ledger in which every key is new.
#[derive(Default)]
pub struct FakeLedger {
    completed: Mutex<Vec<MutationId>>,
}

/// An event log that records every append.
#[derive(Default)]
pub struct FakeEvents {
    appended: Mutex<Vec<(EventScope, NewEvent)>>,
}

/// One server's fakes, shared with the ports it was built from.
pub struct World {
    /// The application.
    pub application: Arc<FakeApplication>,
    /// The event log.
    pub events: Arc<FakeEvents>,
    /// The ledger.
    pub ledger: Arc<FakeLedger>,
    /// The ports to build a server from.
    pub ports: McpPorts,
}

okf_jawn_contract::for_each_operation!(fake_operations);

impl FakeApplication {
    /// Answer `operation` with `response` from now on.
    ///
    /// # Errors
    /// Returns an error when the response table is poisoned.
    pub fn answer_with(&self, operation: &'static str, response: Value) -> Result<(), ApiError> {
        self.responses
            .lock()
            .map_err(|_| poisoned())?
            .insert(operation, response);
        Ok(())
    }

    /// Every call the application received, in order.
    ///
    /// # Errors
    /// Returns an error when the call record is poisoned.
    pub fn calls(&self) -> Result<Vec<Call>, ApiError> {
        Ok(self.calls.lock().map_err(|_| poisoned())?.clone())
    }

    fn answer<T: rmcp::serde::de::DeserializeOwned>(
        &self,
        operation: &'static str,
        context: &OperationContext,
    ) -> Result<T, ApiError> {
        self.calls.lock().map_err(|_| poisoned())?.push(Call {
            operation,
            subject: context.principal.subject.clone(),
        });
        let response = self
            .responses
            .lock()
            .map_err(|_| poisoned())?
            .get(operation)
            .cloned()
            .ok_or_else(|| ApiError::new(ErrorCode::Unavailable, "No test response configured"))?;
        serde_json::from_value(response)
            .map_err(|error| ApiError::new(ErrorCode::Internal, error.to_string()))
    }
}

impl FakeAccess {
    /// Grant `subject` `permissions` on every workspace and on the tenant.
    #[must_use]
    pub fn granting(mut self, subject: &str, permissions: &[Permission]) -> Self {
        self.grants.insert(subject.to_owned(), permissions.to_vec());
        self
    }

    fn of(&self, principal: &Principal) -> Result<Vec<Permission>, ApiError> {
        self.grants
            .get(&principal.subject)
            .cloned()
            .ok_or_else(|| ApiError::new(ErrorCode::Forbidden, "No grant for this subject"))
    }
}

impl AccessControl for FakeAccess {
    fn authorize<'a>(
        &'a self,
        principal: &'a Principal,
        workspace: WorkspaceId,
        _permission: Permission,
    ) -> PortFuture<'a, WorkspaceGrant> {
        Box::pin(async move {
            Ok(WorkspaceGrant {
                scope: StorageScope {
                    tenant_id: principal.tenant_id.clone(),
                    workspace_id: workspace,
                },
                permissions: self.of(principal)?,
            })
        })
    }

    fn authorize_tenant<'a>(
        &'a self,
        principal: &'a Principal,
        _permission: Permission,
    ) -> PortFuture<'a, TenantGrant> {
        Box::pin(async move {
            Ok(TenantGrant {
                tenant_id: principal.tenant_id.clone(),
                permissions: self.of(principal)?,
            })
        })
    }

    fn grants<'a>(&'a self, _principal: &'a Principal) -> PortFuture<'a, Vec<WorkspaceGrant>> {
        Box::pin(async { Ok(Vec::new()) })
    }

    fn grant_creator<'a>(
        &'a self,
        _principal: &'a Principal,
        _workspace: WorkspaceId,
    ) -> PortFuture<'a, WorkspaceGrant> {
        Box::pin(async {
            Err(ApiError::new(
                ErrorCode::Forbidden,
                "The fake grants no creator",
            ))
        })
    }

    fn editors<'a>(&'a self, _scope: &'a StorageScope) -> PortFuture<'a, Vec<String>> {
        Box::pin(async { Ok(Vec::new()) })
    }
}

impl FakeLedger {
    /// Every mutation completed, in order.
    ///
    /// # Errors
    /// Returns an error when the record is poisoned.
    pub fn completed(&self) -> Result<Vec<MutationId>, ApiError> {
        Ok(self.completed.lock().map_err(|_| poisoned())?.clone())
    }
}

impl MutationStore for FakeLedger {
    fn begin<'a>(
        &'a self,
        _key: &'a MutationKey,
        _digest: &'a Digest,
    ) -> PortFuture<'a, BeginOutcome> {
        Box::pin(async {
            Ok(BeginOutcome::New(MutationLease {
                mutation_id: new_mutation_id(),
                token: 1,
            }))
        })
    }

    fn complete(&self, lease: MutationLease, _response: Value) -> PortFuture<'_, ()> {
        Box::pin(async move {
            self.completed
                .lock()
                .map_err(|_| poisoned())?
                .push(lease.mutation_id);
            Ok(())
        })
    }

    fn release(&self, _lease: MutationLease) -> PortFuture<'_, ()> {
        Box::pin(async { Ok(()) })
    }
}

impl FakeEvents {
    /// Every appended event with its scope, in order.
    ///
    /// # Errors
    /// Returns an error when the record is poisoned.
    pub fn appended(&self) -> Result<Vec<(EventScope, NewEvent)>, ApiError> {
        Ok(self.appended.lock().map_err(|_| poisoned())?.clone())
    }
}

impl EventLog for FakeEvents {
    fn append<'a>(
        &'a self,
        scope: &'a EventScope,
        _mutation_id: Option<MutationId>,
        event: NewEvent,
    ) -> PortFuture<'a, Event> {
        Box::pin(async move {
            let kind = serde_json::to_value(&event.kind)
                .map_err(|error| ApiError::new(ErrorCode::Internal, error.to_string()))?;
            self.appended
                .lock()
                .map_err(|_| poisoned())?
                .push((scope.clone(), event));
            serde_json::from_value(json!({
                "id": "1",
                "kind": kind,
                "at": "2026-10-11T00:00:00.000Z"
            }))
            .map_err(|error| ApiError::new(ErrorCode::Internal, error.to_string()))
        })
    }

    fn list<'a>(
        &'a self,
        _scope: &'a EventScope,
        _query: EventQuery,
    ) -> PortFuture<'a, ListEventsResponse> {
        Box::pin(async {
            Ok(ListEventsResponse {
                events: Vec::new(),
                next_cursor: None,
            })
        })
    }
}

impl World {
    /// Fakes in which `access` decides every grant.
    #[must_use]
    pub fn new(access: FakeAccess) -> Self {
        let application = Arc::new(FakeApplication::default());
        let events = Arc::new(FakeEvents::default());
        let ledger = Arc::new(FakeLedger::default());
        let ports = McpPorts {
            application: application.clone(),
            access: Arc::new(access),
            mutations: ledger.clone(),
            events: events.clone(),
        };
        Self {
            application,
            events,
            ledger,
            ports,
        }
    }
}

fn poisoned() -> ApiError {
    ApiError::new(ErrorCode::Internal, "A fake's lock is poisoned")
}
