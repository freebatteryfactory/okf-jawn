//! Route only declared operations through validation, target authorization, and the mutation ledger.
//!
//! Order: validate, decode, `check_rules()`, `targets()`, authorize every target, build the
//! context, `MutationStore::begin`, the handler, then `complete` on success or `release` on
//! error. `check_rules` refuses what no grant can make valid, such as a View bound to another
//! workspace, so its refusal is `InvalidInput` whoever asks.
//! "Error" is anything that fails after `begin` granted the lease, not only the handler: a
//! failure to serialize the response, to build the ledger body, or in `complete` itself also
//! releases the lease, and the caller receives the first error, never the release's.
//! A request that declares no target is refused as `Internal` before any grant is looked up,
//! and so is a draft-bearing, backup, restore or purge operation on a route that is not a human
//! session (`access::check_human_route`, refused as `Forbidden`). Every `Forbidden` from
//! authorization, dispatch's or a handler's own (such as `access::authorize_job_kind`), is
//! recorded as a `permission_denied` event before the refusal is returned: in the log of the
//! refusing workspace target; when no target was evaluated as the refuser (a route refusal, a
//! handler's own), in the tenant's log if the request has a `Deployment` target, else in the
//! workspace's log if its targets name exactly one distinct workspace, else in the tenant's
//! (see `denial_scope`). A grant is used only when it is for the workspace and tenant that were
//! asked for. The ledger never inspects other stores: a
//! resumed attempt re-runs the handler under the same `MutationId`, and what the ledger
//! retains is decided by the request's `ReplayPolicy`.
//!
//! Handlers see the `MutationId`; dispatch keeps the `MutationLease` it was granted and closes
//! the row with it, so an attempt whose lease was taken over cannot complete or release the
//! attempt that now holds the mutation.

use jsonschema::error::ValidationErrorKind;
use okf_jawn_contract::access::Principal;
use okf_jawn_contract::error::{ApiError, ErrorCode, ErrorDetail};
use okf_jawn_contract::identity::{ConnectorId, MutationId, WorkspaceId};
use okf_jawn_contract::metadata::OperationName;
use okf_jawn_contract::scope::{ReplayPolicy, RequestScope, Target};
use schemars::{JsonSchema, generate::SchemaSettings};
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::Value;
use std::sync::OnceLock;
use uuid::Uuid;

use crate::access::{self, AccessControl};
use crate::context::{Attempt, OperationContext, TenantGrant, WorkspaceGrant};
use crate::echo::bounded;
use crate::events::{EventLog, EventScope, NewEvent};
use crate::mutations::{BeginOutcome, MutationKey, MutationLease, MutationStore, request_digest};
use crate::ports::Application;
use crate::storage::StorageScope;

macro_rules! dispatch_operations {
    ($(($id:ident, $request:ty, $response:ty, $path:literal, $label:literal, $alias:literal,
        $operator:literal, $visibility:literal, $permission:ident, $ui:literal, $status:literal,
        $destructive:literal, $description:literal)),* $(,)?) => {
        /// Run one declared operation. Its future holds one state per operation in the table
        /// and is large; [`dispatch`] is the entry point and boxes it.
        async fn run_declared(
            service: &dyn Application,
            ports: &DispatchPorts<'_>,
            caller: &Caller<'_>,
            operation_id: &str,
            input: Value,
        ) -> Result<Value, ApiError> {
            let operation = parse_operation(operation_id)?;
            match operation_id {
                $(stringify!($id) => {
                    static VALIDATOR: OnceLock<Result<jsonschema::Validator, String>> = OnceLock::new();
                    let request: $request = decode_validated(input, &VALIDATOR)?;
                    request.check_rules()?;
                    let mut context =
                        match authorize_targets(ports.access, caller, operation, &request).await {
                            Ok(context) => context,
                            Err(refusal) => {
                                return Err(
                                    record_denial(ports.events, caller.principal, operation, &request, refusal)
                                        .await,
                                );
                            }
                        };
                    let replay = <$request as RequestScope>::REPLAY;
                    match prepare_mutation(ports, &mut context, replay, &request).await? {
                        MutationGate::Run(lease) => {
                            // A handler's own authorization (the job-kind rule) is recorded
                            // like a route refusal: no target refused, so `denial_scope`'s
                            // rule for a request decides the log.
                            let scope = denial_scope(caller.principal, None, &request);
                            let outcome = match service.$id(&context, request).await {
                                Err(error) if error.code == ErrorCode::Forbidden => Err(
                                    record_denied(ports.events, caller.principal, operation, &scope, error)
                                        .await,
                                ),
                                outcome => outcome,
                            };
                            finish(ports.mutations, lease, replay, outcome).await
                        }
                        MutationGate::ShortCircuit(value) => Ok(value),
                    }
                }),*
                _ => Err(ApiError::new(ErrorCode::NotFound, "Unknown operation")),
            }
        }
    };
}

/// The authenticated caller of one dispatch.
#[derive(Debug, Clone, Copy)]
pub struct Caller<'a> {
    /// Server-established identity; never taken from a request body.
    pub principal: &'a Principal,
    /// Browser session; `None` for bearer and connector callers.
    pub session_id: Option<&'a str>,
}

/// Ports dispatch needs beyond the typed application handlers.
pub struct DispatchPorts<'a> {
    /// Grant lookup.
    pub access: &'a dyn AccessControl,
    /// Idempotency ledger.
    pub mutations: &'a dyn MutationStore,
    /// Where a refusal of an authenticated caller is recorded as `permission_denied`.
    pub events: &'a dyn EventLog,
}

enum MutationGate {
    /// Run the handler; a mutation runs under the lease this attempt was granted.
    Run(Option<MutationLease>),
    /// Answer from the ledger without running the handler.
    ShortCircuit(Value),
}

/// An authorization refusal and the target that refused, when one had been reached.
struct Refusal {
    error: ApiError,
    /// `None` when the request was refused before any target was evaluated (its route, or a
    /// request with no target).
    target: Option<Target>,
}

impl Refusal {
    /// A refusal before any target was evaluated.
    const fn before_targets(error: ApiError) -> Self {
        Self {
            error,
            target: None,
        }
    }
}

/// Dispatch one declared JSON operation into its typed implementation.
///
/// The per-operation state is boxed inside, so this future is small and a binding awaits it
/// inline, without `Box::pin`.
///
/// # Errors
/// Returns input, authorization, idempotency, implementation, or serialization errors.
pub async fn dispatch(
    service: &dyn Application,
    ports: &DispatchPorts<'_>,
    caller: &Caller<'_>,
    operation_id: &str,
    input: Value,
) -> Result<Value, ApiError> {
    Box::pin(run_declared(service, ports, caller, operation_id, input)).await
}

async fn prepare_mutation<Req: RequestScope + Serialize>(
    ports: &DispatchPorts<'_>,
    context: &mut OperationContext,
    replay: ReplayPolicy,
    request: &Req,
) -> Result<MutationGate, ApiError> {
    let Some(key) = request.idempotency_key().copied() else {
        return Ok(MutationGate::Run(None));
    };
    let digest = request_digest(request)?;
    let mutation_key = MutationKey {
        tenant_id: context.principal.tenant_id.clone(),
        subject: context.principal.subject.clone(),
        client_id: context.principal.client_id.clone(),
        operation: context.operation,
        key,
    };
    match ports.mutations.begin(&mutation_key, &digest).await? {
        BeginOutcome::New(lease) => {
            context.mutation = Some(lease.mutation_id);
            Ok(MutationGate::Run(Some(lease)))
        }
        BeginOutcome::Abandoned { lease } => {
            context.mutation = Some(lease.mutation_id);
            context.attempt = Attempt::Resumed;
            Ok(MutationGate::Run(Some(lease)))
        }
        BeginOutcome::Replay(stored) => {
            replay_response(replay, stored.body).map(MutationGate::ShortCircuit)
        }
        BeginOutcome::Conflict { operation } => Err(ApiError::new(
            ErrorCode::Conflict,
            "Idempotency key was reused with a different request body",
        )
        .with_detail(ErrorDetail::IdempotencyConflict { operation })),
        BeginOutcome::InProgress {
            mutation_id,
            retry_after,
        } => Err(ApiError::new(
            ErrorCode::InProgress,
            "Another attempt with this idempotency key is still running",
        )
        .with_detail(ErrorDetail::InProgress {
            mutation_id,
            retry_after,
        })),
    }
}

/// Close the ledger row for a handler outcome and return what the caller receives.
///
/// `lease` is the grant this attempt holds. Whatever fails once the handler has run (the
/// handler itself, serializing its response, building the ledger body, or `complete`), the
/// lease is released, so a retry resumes at once instead of waiting the lease out, and the
/// caller receives that first error. The one exception is a lost lease: the store refuses
/// `complete` with `Conflict` because another attempt holds the mutation, and that attempt's
/// lease is not this one's to end.
async fn finish<Res: Serialize>(
    mutations: &dyn MutationStore,
    lease: Option<MutationLease>,
    replay: ReplayPolicy,
    outcome: Result<Res, ApiError>,
) -> Result<Value, ApiError> {
    let Some(lease) = lease else {
        return outcome.and_then(|response| response_body(&response));
    };
    let prepared = outcome.and_then(|response| {
        let body = response_body(&response)?;
        let retained = ledger_body(replay, &body)?;
        Ok((body, retained))
    });
    let error = match prepared {
        Ok((body, retained)) => match mutations.complete(lease, retained).await {
            Ok(()) => return Ok(body),
            Err(lost) if lost.code == ErrorCode::Conflict => return Err(lost),
            Err(error) => error,
        },
        Err(error) => error,
    };
    // Best effort: the caller must see the first error. If the release itself fails, the lease
    // expires into `Abandoned`, which a later attempt resumes safely.
    let _released = mutations.release(lease).await;
    Err(error)
}

fn response_body<Res: Serialize>(response: &Res) -> Result<Value, ApiError> {
    serde_json::to_value(response)
        .map_err(|_| ApiError::new(ErrorCode::Internal, "Response serialization failed"))
}

/// What the ledger retains for a completed mutation, decided by its replay policy.
fn ledger_body(replay: ReplayPolicy, body: &Value) -> Result<Value, ApiError> {
    match replay {
        ReplayPolicy::StoredResponse => Ok(body.clone()),
        ReplayPolicy::AlreadyIssued { id_pointer } => {
            let connector_id = body.pointer(id_pointer).ok_or_else(|| {
                ApiError::new(
                    ErrorCode::Internal,
                    "Issued response is missing the identity its replay policy names",
                )
            })?;
            Ok(serde_json::json!({ "connector_id": connector_id }))
        }
    }
}

fn replay_response(replay: ReplayPolicy, body: Value) -> Result<Value, ApiError> {
    match replay {
        ReplayPolicy::StoredResponse => Ok(body),
        ReplayPolicy::AlreadyIssued { .. } => {
            let connector_id: ConnectorId = body
                .get("connector_id")
                .cloned()
                .ok_or_else(|| {
                    ApiError::new(
                        ErrorCode::Internal,
                        "AlreadyIssued ledger row missing connector_id",
                    )
                })
                .and_then(|value| {
                    serde_json::from_value(value)
                        .map_err(|error| ApiError::new(ErrorCode::Internal, error.to_string()))
                })?;
            Err(ApiError::new(
                ErrorCode::AlreadyIssued,
                "Connector was already issued under this idempotency key",
            )
            .with_detail(ErrorDetail::AlreadyIssued { connector_id }))
        }
    }
}

async fn authorize_targets(
    access: &dyn AccessControl,
    caller: &Caller<'_>,
    operation: OperationName,
    request: &impl RequestScope,
) -> Result<OperationContext, Refusal> {
    let principal = caller.principal;
    let mut grants: Vec<WorkspaceGrant> = Vec::new();
    let mut tenant: Option<TenantGrant> = None;
    let targets = request.targets();
    if targets.is_empty() {
        // Every operation authorizes something, if only sign-in (`Target::Authenticated`). An
        // empty list is a defect in the request's `RequestScope`; running the handler on it
        // would run it with no authorization at all.
        return Err(Refusal::before_targets(ApiError::new(
            ErrorCode::Internal,
            "Request declares no authorization target",
        )));
    }
    // No grant lets an agent route see a draft or back up, restore or purge, so this runs
    // before any grant is looked up.
    access::check_human_route(principal, operation).map_err(Refusal::before_targets)?;
    for target in targets {
        let at = |error| Refusal {
            error,
            target: Some(target),
        };
        match target {
            // Any signed-in principal; the handler filters its result by grants.
            Target::Authenticated => {}
            Target::Deployment(permission) => {
                let raw = access
                    .authorize_tenant(principal, permission)
                    .await
                    .map_err(at)?;
                require_tenant_scope(principal, &raw).map_err(at)?;
                tenant = Some(access::authorize_tenant(principal, raw, permission).map_err(at)?);
            }
            Target::Workspace(workspace, permission) => {
                let raw = access
                    .authorize(principal, workspace, permission)
                    .await
                    .map_err(at)?;
                require_workspace_scope(principal, workspace, &raw).map_err(at)?;
                let grant = access::authorize_workspace(principal, raw, permission).map_err(at)?;
                if !grants
                    .iter()
                    .any(|existing| existing.workspace_id() == workspace)
                {
                    grants.push(grant);
                }
            }
        }
    }
    Ok(OperationContext {
        principal: principal.clone(),
        session_id: caller.session_id.map(str::to_owned),
        operation,
        tenant,
        grants,
        mutation: None,
        attempt: Attempt::First,
    })
}

/// Record a `Forbidden` authorization refusal of an authenticated caller and return the
/// refusal's error unchanged.
///
/// The event goes to the log of the workspace whose target refused, and to the tenant's when a
/// tenant or sign-in target refused (Stage 1b design section 7: "in the workspace's log for a
/// workspace target, tenant-level otherwise"). A refusal by route comes before any target is
/// evaluated; `denial_scope` decides its log from the request.
/// Other errors are returned without an event. The refusal is the answer: a failed append does
/// not turn it into another error, so a caller cannot learn whether the log is writable from
/// the code it receives.
async fn record_denial(
    events: &dyn EventLog,
    principal: &Principal,
    operation: OperationName,
    request: &impl RequestScope,
    refusal: Refusal,
) -> ApiError {
    if refusal.error.code != ErrorCode::Forbidden {
        return refusal.error;
    }
    let scope = denial_scope(principal, refusal.target, request);
    record_denied(events, principal, operation, &scope, refusal.error).await
}

/// Append `permission_denied` for `error` to the log of `scope` and return `error`
/// unchanged, whether or not the append succeeded.
async fn record_denied(
    events: &dyn EventLog,
    principal: &Principal,
    operation: OperationName,
    scope: &EventScope,
    error: ApiError,
) -> ApiError {
    let _recorded = events
        .append(
            scope,
            None,
            NewEvent::permission_denied(principal, operation),
        )
        .await;
    error
}

/// The log a refusal is recorded in.
///
/// A refusal at an evaluated `Target::Workspace` is recorded in that workspace's log; at an
/// evaluated tenant or sign-in target, in the tenant's. A refusal with no evaluated refusing
/// target (a route refusal before targets, and a handler's `Forbidden`) is one rule:
/// tenant-level when the request has any `Target::Deployment`; otherwise the workspace's log
/// when its targets name exactly one distinct workspace; otherwise (none, or two or more
/// distinct workspaces) tenant-level. So a request that is not about one workspace never writes
/// into one workspace's log.
fn denial_scope(
    principal: &Principal,
    target: Option<Target>,
    request: &impl RequestScope,
) -> EventScope {
    let workspace = match target {
        Some(Target::Workspace(workspace_id, _)) => Some(workspace_id),
        Some(Target::Authenticated | Target::Deployment(_)) => None,
        None => sole_workspace(&request.targets()),
    };
    workspace.map_or_else(
        || EventScope::Tenant(principal.tenant_id.clone()),
        |workspace_id| {
            EventScope::Workspace(StorageScope {
                tenant_id: principal.tenant_id.clone(),
                workspace_id,
            })
        },
    )
}

/// The one workspace `targets` name, or `None` when they include a `Deployment` target or name
/// no workspace or more than one.
fn sole_workspace(targets: &[Target]) -> Option<WorkspaceId> {
    let mut named: Option<WorkspaceId> = None;
    for target in targets {
        match *target {
            Target::Deployment(_) => return None,
            Target::Workspace(workspace_id, _) => match named {
                Some(existing) if existing != workspace_id => return None,
                _ => named = Some(workspace_id),
            },
            Target::Authenticated => {}
        }
    }
    named
}

/// Refuse a workspace grant the adapter returned for a workspace or tenant that was not asked.
fn require_workspace_scope(
    principal: &Principal,
    workspace: WorkspaceId,
    grant: &WorkspaceGrant,
) -> Result<(), ApiError> {
    if grant.workspace_id() == workspace && grant.scope.tenant_id == principal.tenant_id {
        Ok(())
    } else {
        Err(foreign_scope())
    }
}

/// Refuse a tenant grant the adapter returned for a tenant other than the caller's.
fn require_tenant_scope(principal: &Principal, grant: &TenantGrant) -> Result<(), ApiError> {
    if grant.tenant_id == principal.tenant_id {
        Ok(())
    } else {
        Err(foreign_scope())
    }
}

fn foreign_scope() -> ApiError {
    ApiError::new(
        ErrorCode::Internal,
        "Access adapter returned a grant for a different scope",
    )
}

fn parse_operation(operation_id: &str) -> Result<OperationName, ApiError> {
    OperationName::ALL
        .iter()
        .copied()
        .find(|name| name.as_str() == operation_id)
        .ok_or_else(|| ApiError::new(ErrorCode::NotFound, "Unknown operation"))
}

fn decode<T: DeserializeOwned>(value: Value) -> Result<T, ApiError> {
    serde_json::from_value(value)
        .map_err(|error| ApiError::new(ErrorCode::InvalidInput, bounded(error.to_string())))
}

fn decode_validated<T: DeserializeOwned + JsonSchema>(
    value: Value,
    cell: &OnceLock<Result<jsonschema::Validator, String>>,
) -> Result<T, ApiError> {
    let validator = cell
        .get_or_init(|| {
            let schema = SchemaSettings::draft2020_12()
                .into_generator()
                .into_root_schema_for::<T>();
            let document = serde_json::to_value(schema).map_err(|error| error.to_string())?;
            jsonschema::validator_for(&document).map_err(|error| error.to_string())
        })
        .as_ref()
        .map_err(|message| ApiError::new(ErrorCode::Internal, message.clone()))?;
    validator
        .validate(&value)
        .map_err(|error| invalid_input(&error))?;
    decode(value)
}

/// Report a schema failure and name the offending field as a JSON Pointer into the request.
///
/// A missing required property is reported at the parent object, so its name is appended.
/// The validator's text quotes the offending value, so it is [`bounded`].
fn invalid_input(error: &jsonschema::ValidationError<'_>) -> ApiError {
    let location = if let ValidationErrorKind::Required {
        property: Value::String(name),
    } = error.kind()
    {
        error.instance_path().join(name)
    } else {
        error.instance_path().clone()
    };
    let refused = ApiError::new(ErrorCode::InvalidInput, bounded(error.to_string()));
    if location.is_empty() {
        refused
    } else {
        refused.with_field(location.as_str())
    }
}

/// Allocate a fresh mutation identity for adapters and fixtures.
#[must_use]
pub fn new_mutation_id() -> MutationId {
    MutationId(Uuid::new_v4())
}

okf_jawn_contract::for_each_operation!(dispatch_operations);

#[cfg(test)]
mod tests {
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use okf_jawn_contract::access::{AccessRoute, Permission, Principal};
    use okf_jawn_contract::error::{ApiError, ErrorCode, ErrorDetail};
    use okf_jawn_contract::identity::{Digest, IdempotencyKey, TenantId, WorkspaceId};
    use okf_jawn_contract::metadata::OperationName;
    use okf_jawn_contract::scope::{ReplayPolicy, RequestScope, Target};
    use serde::{Serialize, Serializer};
    use serde_json::{Value, json};

    use super::{Caller, authorize_targets, finish, ledger_body, new_mutation_id, replay_response};
    use crate::access::AccessControl;
    use crate::context::{TenantGrant, WorkspaceGrant};
    use crate::mutations::{BeginOutcome, MutationKey, MutationLease, MutationStore};
    use crate::ports::PortFuture;
    use crate::storage::StorageScope;

    /// Access double that grants nothing and counts how often it was asked.
    #[derive(Default)]
    struct NoAccess {
        lookups: AtomicUsize,
    }

    /// A request whose `RequestScope` declares nothing to authorize. No contract request does;
    /// the dispatch table only reaches contract requests, so this one is checked directly.
    struct Untargeted;

    /// Ledger double that records the bodies it was asked to keep and the leases it was asked
    /// to end. `begin` is never reached: these tests enter after the handler.
    #[derive(Default)]
    struct RecordingLedger {
        completed: Mutex<Vec<Value>>,
        released: Mutex<Vec<MutationLease>>,
    }

    /// A handler response whose serialization fails.
    struct Unserializable;

    const CONNECTOR: &str = "cccccccc-cccc-4ccc-8ccc-cccccccccccc";
    const POLICY: ReplayPolicy = ReplayPolicy::AlreadyIssued {
        id_pointer: "/issued/id",
    };

    impl NoAccess {
        fn refuse<T>(&self) -> Result<T, ApiError> {
            self.lookups.fetch_add(1, Ordering::SeqCst);
            Err(ApiError::new(ErrorCode::Forbidden, "nothing is granted"))
        }
    }

    impl AccessControl for NoAccess {
        fn authorize<'a>(
            &'a self,
            _principal: &'a Principal,
            _workspace: WorkspaceId,
            _permission: Permission,
        ) -> PortFuture<'a, WorkspaceGrant> {
            Box::pin(async { self.refuse() })
        }

        fn authorize_tenant<'a>(
            &'a self,
            _principal: &'a Principal,
            _permission: Permission,
        ) -> PortFuture<'a, TenantGrant> {
            Box::pin(async { self.refuse() })
        }

        fn grants<'a>(&'a self, _principal: &'a Principal) -> PortFuture<'a, Vec<WorkspaceGrant>> {
            Box::pin(async { self.refuse() })
        }

        fn grant_creator<'a>(
            &'a self,
            _principal: &'a Principal,
            _workspace: WorkspaceId,
        ) -> PortFuture<'a, WorkspaceGrant> {
            Box::pin(async { self.refuse() })
        }

        fn editors<'a>(&'a self, _scope: &'a StorageScope) -> PortFuture<'a, Vec<String>> {
            Box::pin(async { self.refuse() })
        }
    }

    impl RequestScope for Untargeted {
        fn targets(&self) -> Vec<Target> {
            Vec::new()
        }

        fn idempotency_key(&self) -> Option<&IdempotencyKey> {
            None
        }
    }

    impl RecordingLedger {
        fn completed(&self) -> Result<Vec<Value>, ApiError> {
            Ok(self.completed.lock().map_err(|_| poisoned())?.clone())
        }

        fn released(&self) -> Result<Vec<MutationLease>, ApiError> {
            Ok(self.released.lock().map_err(|_| poisoned())?.clone())
        }
    }

    impl MutationStore for RecordingLedger {
        fn begin<'a>(
            &'a self,
            _key: &'a MutationKey,
            _digest: &'a Digest,
        ) -> PortFuture<'a, BeginOutcome> {
            Box::pin(async {
                Err(ApiError::new(
                    ErrorCode::Internal,
                    "the recording ledger does not begin mutations",
                ))
            })
        }

        fn complete(&self, _lease: MutationLease, response: Value) -> PortFuture<'_, ()> {
            Box::pin(async move {
                self.completed
                    .lock()
                    .map_err(|_| poisoned())?
                    .push(response);
                Ok(())
            })
        }

        fn release(&self, lease: MutationLease) -> PortFuture<'_, ()> {
            Box::pin(async move {
                self.released.lock().map_err(|_| poisoned())?.push(lease);
                Ok(())
            })
        }
    }

    impl Serialize for Unserializable {
        fn serialize<S: Serializer>(&self, _serializer: S) -> Result<S::Ok, S::Error> {
            Err(serde::ser::Error::custom(
                "this response cannot be serialized",
            ))
        }
    }

    fn poisoned() -> ApiError {
        ApiError::new(ErrorCode::Internal, "recording ledger lock poisoned")
    }

    fn lease() -> MutationLease {
        MutationLease {
            mutation_id: new_mutation_id(),
            token: 7,
        }
    }

    #[tokio::test]
    async fn a_request_that_declares_no_target_is_refused_as_internal() -> Result<(), ApiError> {
        let access = NoAccess::default();
        let principal = Principal {
            subject: "alice".to_owned(),
            tenant_id: TenantId::try_from("tenant-local".to_owned())
                .map_err(|error| ApiError::new(ErrorCode::Internal, error.to_string()))?,
            route: AccessRoute::LocalOwner,
            client_id: None,
            delegation: None,
        };
        let caller = Caller {
            principal: &principal,
            session_id: None,
        };
        let refused =
            authorize_targets(&access, &caller, OperationName::GetHealth, &Untargeted).await;
        assert!(matches!(refused, Err(refusal) if refusal.error.code == ErrorCode::Internal));
        assert_eq!(access.lookups.load(Ordering::SeqCst), 0);
        Ok(())
    }

    #[tokio::test]
    async fn an_unbuildable_ledger_body_releases_the_lease_and_stores_nothing()
    -> Result<(), ApiError> {
        let ledger = RecordingLedger::default();
        let held = lease();
        // The handler succeeded and returned a secret, but not the identity the policy names.
        let outcome: Result<Value, ApiError> = Ok(json!({ "secret": "s" }));
        let refused = finish(&ledger, Some(held), POLICY, outcome).await;
        assert!(matches!(refused, Err(error) if error.code == ErrorCode::Internal));
        assert_eq!(ledger.released()?, vec![held]);
        assert_eq!(ledger.completed()?, Vec::<Value>::new());
        Ok(())
    }

    #[tokio::test]
    async fn an_unserializable_response_releases_the_lease_and_stores_nothing()
    -> Result<(), ApiError> {
        let ledger = RecordingLedger::default();
        let held = lease();
        let outcome: Result<Unserializable, ApiError> = Ok(Unserializable);
        let refused = finish(&ledger, Some(held), ReplayPolicy::StoredResponse, outcome).await;
        assert!(matches!(refused, Err(error) if error.code == ErrorCode::Internal));
        assert_eq!(ledger.released()?, vec![held]);
        assert_eq!(ledger.completed()?, Vec::<Value>::new());
        Ok(())
    }

    #[tokio::test]
    async fn a_completed_mutation_keeps_its_ledger_body_and_is_not_released() -> Result<(), ApiError>
    {
        let ledger = RecordingLedger::default();
        let body = json!({ "issued": { "id": CONNECTOR }, "secret": "s" });
        let outcome: Result<Value, ApiError> = Ok(body.clone());
        assert_eq!(finish(&ledger, Some(lease()), POLICY, outcome).await?, body);
        assert_eq!(
            ledger.completed()?,
            vec![json!({ "connector_id": CONNECTOR })]
        );
        assert_eq!(ledger.released()?, Vec::new());
        Ok(())
    }

    #[test]
    fn stored_response_policy_keeps_the_whole_body() -> Result<(), ApiError> {
        let body = json!({ "revision": "r", "warnings": [] });
        assert_eq!(ledger_body(ReplayPolicy::StoredResponse, &body)?, body);
        Ok(())
    }

    #[test]
    fn already_issued_policy_keeps_only_the_id_at_its_pointer() -> Result<(), ApiError> {
        let body = json!({ "issued": { "id": CONNECTOR, "label": "agent" }, "secret": "s" });
        assert_eq!(
            ledger_body(POLICY, &body)?,
            json!({ "connector_id": CONNECTOR })
        );
        Ok(())
    }

    #[test]
    fn already_issued_policy_refuses_a_response_without_the_id() {
        let refused = ledger_body(POLICY, &json!({ "secret": "s" }));
        assert!(matches!(refused, Err(error) if error.code == ErrorCode::Internal));
    }

    #[test]
    fn already_issued_replay_names_the_connector_and_returns_no_body() {
        let replayed = replay_response(POLICY, json!({ "connector_id": CONNECTOR }));
        assert!(matches!(
            replayed,
            Err(error) if error.code == ErrorCode::AlreadyIssued
                && matches!(
                    error.detail.as_deref(),
                    Some(ErrorDetail::AlreadyIssued { connector_id })
                        if connector_id.0.to_string() == CONNECTOR
                )
        ));
    }
}
