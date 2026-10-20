//! Fakes of the ports an MCP server holds, built for this crate's tests; core's are core's.
//!
//! `FakeApplication` answers each operation from a configured response, and `get_object` from
//! a configured outcome per object digest first, and records who called what with which
//! request, and the whole principal of each call's context. `FakeAccess` grants each known subject a fixed permission set on every workspace
//! but the ones it refuses, and on the tenant, so dispatch's own route and permission rules
//! decide. `FakeLedger` always begins a new mutation. `FakeEvents` records every append. No
//! workspace is archived here: `FakeApplication` answers every archive question "live", and
//! the archive rule is core dispatch's, tested in core.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex};

use okf_jawn_contract::access::{Permission, Principal};
use okf_jawn_contract::error::{ApiError, ErrorCode};
use okf_jawn_contract::events::{Event, ListEventsResponse};
use okf_jawn_contract::identity::{Digest, MutationId, Timestamp, WorkspaceId};
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
            $(fn $id<'a>(&'a self, context: &'a OperationContext, request: $request)
                -> PortFuture<'a, $response> {
                Box::pin(async move { self.answer(stringify!($id), context, &request) })
            })*

            /// Every workspace is live: no test of this crate archives one.
            fn archived_at<'a>(&'a self, _scope: &'a StorageScope)
                -> PortFuture<'a, Option<Timestamp>> {
                Box::pin(async { Ok(None) })
            }
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
    objects: Mutex<BTreeMap<String, Result<Value, ApiError>>>,
    calls: Mutex<Vec<Call>>,
    requests: Mutex<Vec<(&'static str, Value)>>,
    callers: Mutex<Vec<(&'static str, Value)>>,
}

/// Grants by subject; an unknown subject holds nothing, and nobody holds a refused workspace.
#[derive(Default)]
pub struct FakeAccess {
    grants: BTreeMap<String, Vec<Permission>>,
    refused: BTreeSet<String>,
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

    /// Answer `get_object` for the object `digest` with `outcome` from now on, before any
    /// response configured for `get_object` as a whole.
    ///
    /// # Errors
    /// Returns an error when the object table is poisoned.
    pub fn serve_object(
        &self,
        digest: &str,
        outcome: Result<Value, ApiError>,
    ) -> Result<(), ApiError> {
        self.objects
            .lock()
            .map_err(|_| poisoned())?
            .insert(digest.to_owned(), outcome);
        Ok(())
    }

    /// Every call the application received, in order.
    ///
    /// # Errors
    /// Returns an error when the call record is poisoned.
    pub fn calls(&self) -> Result<Vec<Call>, ApiError> {
        Ok(self.calls.lock().map_err(|_| poisoned())?.clone())
    }

    /// The serialized requests of every call to `operation`, in order.
    ///
    /// # Errors
    /// Returns an error when the request record is poisoned.
    pub fn requests(&self, operation: &str) -> Result<Vec<Value>, ApiError> {
        Ok(self
            .requests
            .lock()
            .map_err(|_| poisoned())?
            .iter()
            .filter(|(called, _)| *called == operation)
            .map(|(_, request)| request.clone())
            .collect())
    }

    /// The whole principal, serialized, of the context of every call to `operation`, in order:
    /// its subject, tenant, route, client and delegation ceiling as dispatch handed them over.
    ///
    /// # Errors
    /// Returns an error when the caller record is poisoned.
    pub fn callers(&self, operation: &str) -> Result<Vec<Value>, ApiError> {
        Ok(self
            .callers
            .lock()
            .map_err(|_| poisoned())?
            .iter()
            .filter(|(called, _)| *called == operation)
            .map(|(_, principal)| principal.clone())
            .collect())
    }

    fn answer<T: rmcp::serde::de::DeserializeOwned, R: rmcp::serde::Serialize>(
        &self,
        operation: &'static str,
        context: &OperationContext,
        request: &R,
    ) -> Result<T, ApiError> {
        let request = serde_json::to_value(request)
            .map_err(|error| ApiError::new(ErrorCode::Internal, error.to_string()))?;
        let caller = serde_json::to_value(&context.principal)
            .map_err(|error| ApiError::new(ErrorCode::Internal, error.to_string()))?;
        self.callers
            .lock()
            .map_err(|_| poisoned())?
            .push((operation, caller));
        self.calls.lock().map_err(|_| poisoned())?.push(Call {
            operation,
            subject: context.principal.subject.clone(),
        });
        self.requests
            .lock()
            .map_err(|_| poisoned())?
            .push((operation, request.clone()));
        let object = request
            .get("object")
            .and_then(Value::as_str)
            .filter(|_| operation == "get_object");
        let served = match object {
            Some(digest) => self
                .objects
                .lock()
                .map_err(|_| poisoned())?
                .get(digest)
                .cloned(),
            None => None,
        };
        let response = match served {
            Some(outcome) => outcome?,
            None => self
                .responses
                .lock()
                .map_err(|_| poisoned())?
                .get(operation)
                .cloned()
                .ok_or_else(|| {
                    ApiError::new(ErrorCode::Unavailable, "No test response configured")
                })?,
        };
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

    /// Refuse every subject on `workspace`, whatever it is granted elsewhere.
    #[must_use]
    pub fn refusing(mut self, workspace: &str) -> Self {
        self.refused.insert(workspace.to_owned());
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
            if self.refused.contains(&workspace.0.to_string()) {
                return Err(ApiError::new(
                    ErrorCode::Forbidden,
                    "No grant on this workspace",
                ));
            }
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
        _workspace: Option<WorkspaceId>,
    ) -> PortFuture<'a, BeginOutcome> {
        Box::pin(async {
            Ok(BeginOutcome::New(MutationLease {
                mutation_id: new_mutation_id(),
                token: 1,
            }))
        })
    }

    fn complete(
        &self,
        lease: MutationLease,
        _response: Value,
        _workspace: Option<WorkspaceId>,
    ) -> PortFuture<'_, ()> {
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
